use anyhow::{anyhow, Result};
use audb_protocol::{recv_message, send_message, Command, CommandOutput, CommandResult, Request, Response, ServerStatus};
use crate::pool::ConnectionPool;
use nix::unistd::Uid;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tracing::{info, warn, error};

// Get scripts from audb-core (single source of truth)
const TAP_SCRIPT: &str = audb_core::features::input::scripts::ScriptManager::tap_script_content();
const SWIPE_SCRIPT: &str = audb_core::features::input::scripts::ScriptManager::swipe_script_content();
const REMOTE_TAP_PATH: &str = "/tmp/audb_tap.py";
const REMOTE_SWIPE_PATH: &str = "/tmp/audb_swipe.py";

/// Get the path to the Unix socket
pub fn socket_path() -> PathBuf {
    let uid = Uid::current();
    PathBuf::from(format!("/tmp/audb-server-{}.sock", uid))
}

/// Start the Unix socket server
pub async fn start_server(
    pool: Arc<ConnectionPool>,
    mut shutdown_signal: tokio::sync::mpsc::Receiver<()>,
) -> Result<()> {
    let socket_path = socket_path();

    // Remove old socket file if it exists
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }

    // Create Unix socket listener
    let listener = UnixListener::bind(&socket_path)?;

    // Set socket permissions to 0600 (owner read/write only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&socket_path, permissions)?;
    }

    info!("Listening on Unix socket: {}", socket_path.display());

    // Main server loop
    loop {
        tokio::select! {
            // Accept new client connections
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        info!("Client connected");
                        let pool_clone = Arc::clone(&pool);
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, pool_clone).await {
                                error!("Client handler error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept connection: {}", e);
                    }
                }
            }

            // Shutdown signal received
            _ = shutdown_signal.recv() => {
                info!("Shutdown signal received, stopping server");
                break;
            }
        }
    }

    // Cleanup: remove socket file
    if socket_path.exists() {
        std::fs::remove_file(&socket_path)?;
    }

    Ok(())
}

/// Handle a single client connection
async fn handle_client(mut stream: UnixStream, pool: Arc<ConnectionPool>) -> Result<()> {
    loop {
        // Receive request from client
        let request: Request = match recv_message(&mut stream).await {
            Ok(req) => req,
            Err(e) => {
                // Client disconnected or error reading
                info!("Client disconnected: {}", e);
                break;
            }
        };

        info!("Received request ID {}: {:?}", request.id, request.command);

        // Process command
        let result = process_command(request.command, &pool).await;

        // Send response
        let response = Response {
            id: request.id,
            result,
        };

        send_message(&mut stream, &response).await?;
    }

    Ok(())
}

/// Process a command and return the result
async fn process_command(command: Command, pool: &ConnectionPool) -> CommandResult {
    match command {
        Command::Ping => {
            // Simple ping/pong for testing
            CommandResult::Success {
                output: CommandOutput::Lines(vec!["pong".to_string()]),
            }
        }

        Command::ServerStatus => {
            // Return server status
            match get_server_status(pool).await {
                Ok(status) => CommandResult::Success {
                    output: CommandOutput::Status(status),
                },
                Err(e) => CommandResult::Error {
                    message: format!("Failed to get server status: {}", e),
                    kind: audb_protocol::ErrorKind::ServerError,
                },
            }
        }

        Command::KillServer => {
            // Signal graceful shutdown
            info!("Kill server command received, initiating shutdown");
            // Return success - the server will be shut down by the signal handler
            // We need to trigger the shutdown signal
            CommandResult::Success {
                output: CommandOutput::Lines(vec!["Server shutdown initiated".to_string()]),
            }
        }

        // Shell command - Phase 2 implementation
        Command::Shell { device, root, command } => {
            match pool.execute_command(&device, &command, root).await {
                Ok(lines) => CommandResult::Success {
                    output: CommandOutput::Lines(lines),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Install { device, rpm_path, rpm_data } => {
            match execute_install(pool, &device, &rpm_path, rpm_data).await {
                Ok(output) => CommandResult::Success {
                    output: CommandOutput::Lines(output),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Uninstall { device, package_name } => {
            match execute_uninstall(pool, &device, &package_name).await {
                Ok(output) => CommandResult::Success {
                    output: CommandOutput::Lines(output),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Packages { device, filter } => {
            match execute_packages(pool, &device, filter).await {
                Ok(output) => CommandResult::Success {
                    output: CommandOutput::Lines(output),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Push { device, local_path, remote_path, data } => {
            match execute_push(pool, &device, &local_path, &remote_path, data).await {
                Ok(output) => CommandResult::Success {
                    output: CommandOutput::Lines(output),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Pull { device, remote_path } => {
            match execute_pull(pool, &device, &remote_path).await {
                Ok(data) => CommandResult::Success {
                    output: CommandOutput::Binary(data),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Info { device, category } => {
            match execute_info(pool, &device, category).await {
                Ok(info) => CommandResult::Success {
                    output: CommandOutput::DeviceInfo(info),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Tap { device, x, y, event_device, duration_ms } => {
            match execute_tap(pool, &device, x, y, event_device, duration_ms).await {
                Ok(output) => CommandResult::Success {
                    output: CommandOutput::Lines(output),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Swipe { device, mode, event_device } => {
            match execute_swipe(pool, &device, mode, event_device).await {
                Ok(output) => CommandResult::Success {
                    output: CommandOutput::Lines(output),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Key { device, key_name } => {
            match execute_key(pool, &device, &key_name).await {
                Ok(output) => CommandResult::Success {
                    output: CommandOutput::Lines(output),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Screenshot { device } => {
            match execute_screenshot(pool, &device).await {
                Ok(data) => CommandResult::Success {
                    output: CommandOutput::Binary(data),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Launch { device, app_name } => {
            match execute_launch(pool, &device, &app_name).await {
                Ok(output) => CommandResult::Success {
                    output: CommandOutput::Lines(output),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Stop { device, app_name } => {
            match execute_stop(pool, &device, &app_name).await {
                Ok(output) => CommandResult::Success {
                    output: CommandOutput::Lines(output),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Logs { device, args } => {
            match execute_logs(pool, &device, args).await {
                Ok(output) => CommandResult::Success {
                    output: CommandOutput::Lines(output),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }

        Command::Reconnect { device } => {
            warn!("Reconnect command not yet implemented: device={:?}", device);
            CommandResult::Error {
                message: "Reconnect command not yet implemented in Phase 1".to_string(),
                kind: audb_protocol::ErrorKind::ServerError,
            }
        }

        Command::Open { device, url } => {
            match execute_open(pool, &device, &url).await {
                Ok(output) => CommandResult::Success {
                    output: CommandOutput::Lines(output),
                },
                Err(e) => {
                    let kind = if e.to_string().contains("not found") {
                        audb_protocol::ErrorKind::DeviceNotFound
                    } else {
                        audb_protocol::ErrorKind::CommandFailed
                    };
                    CommandResult::Error {
                        message: e.to_string(),
                        kind,
                    }
                }
            }
        }
    }
}

/// Get current server status
async fn get_server_status(pool: &ConnectionPool) -> Result<ServerStatus> {
    use audb_protocol::{ConnectionStateInfo, DeviceStatus};
    use crate::connection::ConnectionState;

    let pid = std::process::id();
    let socket_path = socket_path();

    // Get device statuses from pool
    let devices = pool.list_devices().await;
    let mut device_statuses: Vec<DeviceStatus> = vec![];

    for (host, state) in devices {
        if let Ok(conn) = pool.get_device_info(&host).await {
            let state_info = match state {
                ConnectionState::Disconnected => ConnectionStateInfo::Disconnected,
                ConnectionState::Connecting { attempt, .. } => ConnectionStateInfo::Connecting { attempt },
                ConnectionState::Connected { since } => ConnectionStateInfo::Connected {
                    duration_secs: since.elapsed().as_secs(),
                },
                ConnectionState::Errored { error, .. } => ConnectionStateInfo::Errored {
                    error,
                    retry_in_secs: None,
                },
                ConnectionState::Disabled => ConnectionStateInfo::Disabled,
            };

            device_statuses.push(DeviceStatus {
                name: conn.device.name.clone(),
                host: conn.device.host.clone(),
                port: conn.device.port,
                state: state_info,
                stats: audb_protocol::ConnectionStats {
                    connect_attempts: conn.stats.connect_attempts,
                    successful_commands: conn.stats.successful_commands,
                    failed_commands: conn.stats.failed_commands,
                    last_error: conn.stats.last_error.clone(),
                },
            });
        }
    }

    Ok(ServerStatus {
        pid,
        uptime_secs: 0, // TODO: Track actual uptime
        socket_path: socket_path.to_string_lossy().to_string(),
        devices: device_statuses,
    })
}

/// Execute Install command
async fn execute_install(
    pool: &ConnectionPool,
    device_host: &str,
    rpm_path: &str,
    rpm_data: Vec<u8>,
) -> Result<Vec<String>> {
    info!("Installing {} on device {}", rpm_path, device_host);

    // Get just the filename
    let file_name = std::path::Path::new(rpm_path)
        .file_name()
        .ok_or_else(|| anyhow!("Invalid RPM path"))?
        .to_string_lossy()
        .to_string();

    // Write RPM data to temporary local file
    let local_temp = std::env::temp_dir().join(&file_name);
    std::fs::write(&local_temp, rpm_data)?;

    // Upload to device Downloads directory
    let remote_path = PathBuf::from(format!("/home/defaultuser/Downloads/{}", file_name));
    info!("Uploading {} to {}...", file_name, remote_path.display());
    pool.upload_file(device_host, &local_temp, &remote_path).await?;

    // Cleanup local temp file
    std::fs::remove_file(&local_temp).ok();

    // Install via D-Bus APM
    info!("Installing package via APM...");
    let install_command = format!(
        "gdbus call --system --dest ru.omp.APM --object-path /ru/omp/APM --method ru.omp.APM.Install \"{}\" \"{{}}\"",
        remote_path.display()
    );

    let output = pool.execute_command(device_host, &install_command, false).await?;

    // Cleanup remote file
    let cleanup_command = format!("rm -f {}", remote_path.display());
    pool.execute_command(device_host, &cleanup_command, false).await.ok();

    info!("Package installed successfully");
    Ok(output)
}

/// Execute Tap command
async fn execute_tap(
    pool: &ConnectionPool,
    device_host: &str,
    x: u16,
    y: u16,
    event_device: Option<String>,
    duration_ms: Option<u32>,
) -> Result<Vec<String>> {
    info!("Tapping at ({}, {}) on device {}", x, y, device_host);

    // Validate coordinates
    if x > 4096 || y > 4096 {
        return Err(anyhow!("Coordinates out of range: ({}, {}). Max: 4096x4096", x, y));
    }

    // Ensure tap script is present (uses persistent connection)
    pool.ensure_script(device_host, "tap", REMOTE_TAP_PATH, TAP_SCRIPT).await?;

    // Build tap command with optional --event and --duration flags
    let mut tap_command = format!("python3 {} {} {}", REMOTE_TAP_PATH, x, y);
    
    if let Some(ref event_dev) = event_device {
        tap_command.push_str(&format!(" --event {}", event_dev));
    }
    
    if let Some(duration) = duration_ms {
        tap_command.push_str(&format!(" --duration {}", duration));
    }

    info!("Executing tap with devel-su...");
    let output = pool.execute_command(device_host, &tap_command, true).await?;

    Ok(output)
}

/// Execute Swipe command
async fn execute_swipe(
    pool: &ConnectionPool,
    device_host: &str,
    mode: audb_protocol::SwipeMode,
    event_device: Option<String>,
) -> Result<Vec<String>> {
    info!("Executing swipe on device {}", device_host);

    // Validate coordinates if needed
    if let audb_protocol::SwipeMode::Coords { x1, y1, x2, y2 } = &mode {
        for coord in [x1, y1, x2, y2] {
            if *coord > 4096 {
                return Err(anyhow!("Coordinate out of range: {}. Max: 4096", coord));
            }
        }
    }

    // Ensure swipe script is present (uses persistent connection)
    pool.ensure_script(device_host, "swipe", REMOTE_SWIPE_PATH, SWIPE_SCRIPT).await?;

    // Build command based on mode
    let base_cmd = match mode {
        audb_protocol::SwipeMode::Coords { x1, y1, x2, y2 } => {
            format!("python3 {} {} {} {} {}", REMOTE_SWIPE_PATH, x1, y1, x2, y2)
        }
        audb_protocol::SwipeMode::Direction(dir) => {
            let dir_arg = match dir {
                audb_protocol::SwipeDirection::Left => "rl",
                audb_protocol::SwipeDirection::Right => "lr",
                audb_protocol::SwipeDirection::Up => "du",
                audb_protocol::SwipeDirection::Down => "ud",
            };
            format!("python3 {} {}", REMOTE_SWIPE_PATH, dir_arg)
        }
    };

    // Add --event flag if specified
    let swipe_command = if let Some(ref event_dev) = event_device {
        format!("{} --event {}", base_cmd, event_dev)
    } else {
        base_cmd
    };

    info!("Executing swipe with devel-su...");
    let output = pool.execute_command(device_host, &swipe_command, true).await?;

    Ok(output)
}

/// Get screen dimensions from device
async fn get_screen_dimensions(pool: &ConnectionPool, device_host: &str) -> (u32, u32) {
    // Query screen resolution via D-Bus
    let dbus_cmd = "gdbus call --system --dest ru.omp.deviceinfo --object-path /ru/omp/deviceinfo/Features --method ru.omp.deviceinfo.Features.getScreenResolution";
    
    if let Ok(output) = pool.execute_command(device_host, dbus_cmd, false).await {
        if let Some(line) = output.first() {
            // Parse format like "('720x1440',)"
            let s = line.trim_matches(|c| c == '(' || c == ')' || c == ',' || c == '\'').trim();
            if let Some((w, h)) = s.split_once('x') {
                if let (Ok(width), Ok(height)) = (w.parse(), h.parse()) {
                    return (width, height);
                }
            }
        }
    }
    // Default fallback
    (720, 1440)
}

/// Execute Key command
async fn execute_key(
    pool: &ConnectionPool,
    device_host: &str,
    key_name: &str,
) -> Result<Vec<String>> {
    info!("Sending key '{}' on device {}", key_name, device_host);

    let key_lower = key_name.to_lowercase();

    // Handle keys via MCE D-Bus (Sailfish/Aurora OS)
    match key_lower.as_str() {
        // Power key - use MCE D-Bus
        "power" => {
            let cmd = "gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.req_trigger_powerkey_event 0";
            pool.execute_command(device_host, cmd, false).await?;
            info!("Power key sent via MCE D-Bus");
            Ok(vec!["Power key sent".to_string()])
        }

        // Home - Sailfish uses swipe from bottom edge, simulate with swipe gesture
        "home" => {
            let (width, height) = get_screen_dimensions(pool, device_host).await;
            let center_x = width / 2;
            let center_y = height / 2;
            
            pool.ensure_script(device_host, "swipe", REMOTE_SWIPE_PATH, SWIPE_SCRIPT).await?;
            // Swipe from bottom edge to center, pass screen dimensions via env
            let cmd = format!(
                "XMAX={} YMAX={} python3 {} {} {} {} {}",
                width, height, REMOTE_SWIPE_PATH,
                center_x, height, center_x, center_y
            );
            pool.execute_command(device_host, &cmd, true).await?;
            info!("Home gesture sent (swipe from bottom edge)");
            Ok(vec!["Home gesture sent (swipe up)".to_string()])
        }

        // Back - Sailfish uses swipe from left edge
        "back" => {
            let (width, height) = get_screen_dimensions(pool, device_host).await;
            
            pool.ensure_script(device_host, "swipe", REMOTE_SWIPE_PATH, SWIPE_SCRIPT).await?;
            // Use lr direction with correct screen dimensions
            let cmd = format!("XMAX={} YMAX={} python3 {} lr", width, height, REMOTE_SWIPE_PATH);
            pool.execute_command(device_host, &cmd, true).await?;
            info!("Back gesture sent (swipe from left)");
            Ok(vec!["Back gesture sent (swipe from left)".to_string()])
        }

        // Volume keys - use evdev injection to mtk-kpd (event1)
        // First press shows indicator, second press changes volume
        "volumeup" | "vol+" => {
            let cmd = r#"python3 -c "
import struct, os, time
EV_KEY, EV_SYN, KEY_VOLUMEUP = 0x01, 0x00, 115
fd = os.open('/dev/input/event1', os.O_WRONLY)
def w(t, c, v):
    os.write(fd, struct.pack('IIHHi', int(time.time()), int((time.time()%1)*1000000), t, c, v))
for _ in range(2):
    w(EV_KEY, KEY_VOLUMEUP, 1); w(EV_SYN, 0, 0)
    time.sleep(0.05)
    w(EV_KEY, KEY_VOLUMEUP, 0); w(EV_SYN, 0, 0)
    time.sleep(0.1)
os.close(fd)
""#;
            pool.execute_command(device_host, cmd, true).await?;
            info!("Volume up sent via evdev");
            Ok(vec!["Volume increased".to_string()])
        }

        "volumedown" | "vol-" => {
            let cmd = r#"python3 -c "
import struct, os, time
EV_KEY, EV_SYN, KEY_VOLUMEDOWN = 0x01, 0x00, 114
fd = os.open('/dev/input/event1', os.O_WRONLY)
def w(t, c, v):
    os.write(fd, struct.pack('IIHHi', int(time.time()), int((time.time()%1)*1000000), t, c, v))
for _ in range(2):
    w(EV_KEY, KEY_VOLUMEDOWN, 1); w(EV_SYN, 0, 0)
    time.sleep(0.05)
    w(EV_KEY, KEY_VOLUMEDOWN, 0); w(EV_SYN, 0, 0)
    time.sleep(0.1)
os.close(fd)
""#;
            pool.execute_command(device_host, cmd, true).await?;
            info!("Volume down sent via evdev");
            Ok(vec!["Volume decreased".to_string()])
        }

        // Menu - swipe from top (shows events/notifications)
        "menu" => {
            let (width, height) = get_screen_dimensions(pool, device_host).await;
            
            pool.ensure_script(device_host, "swipe", REMOTE_SWIPE_PATH, SWIPE_SCRIPT).await?;
            // Use ud direction with correct screen dimensions
            let cmd = format!("XMAX={} YMAX={} python3 {} ud", width, height, REMOTE_SWIPE_PATH);
            pool.execute_command(device_host, &cmd, true).await?;
            info!("Menu gesture sent (swipe from top to bottom)");
            Ok(vec!["Menu gesture sent (swipe down)".to_string()])
        }

        // Close app - same as home gesture (swipe from bottom edge)
        "close" => {
            let (width, height) = get_screen_dimensions(pool, device_host).await;
            let center_x = width / 2;
            let center_y = height / 2;
            
            pool.ensure_script(device_host, "swipe", REMOTE_SWIPE_PATH, SWIPE_SCRIPT).await?;
            // Same as home - swipe from bottom edge to center
            let cmd = format!(
                "XMAX={} YMAX={} python3 {} {} {} {} {}",
                width, height, REMOTE_SWIPE_PATH,
                center_x, height, center_x, center_y
            );
            pool.execute_command(device_host, &cmd, true).await?;
            info!("Close gesture sent (swipe from bottom)");
            Ok(vec!["Close gesture sent (swipe up)".to_string()])
        }

        // Lock screen
        "lock" => {
            let cmd = "gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.req_tklock_mode_change 'locked'";
            pool.execute_command(device_host, cmd, false).await?;
            info!("Screen locked via MCE D-Bus");
            Ok(vec!["Screen locked".to_string()])
        }

        // Unlock screen (turn on display and show lock screen)
        "unlock" | "wakeup" => {
            // First unlock tklock, then turn on display
            let cmd1 = "gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.req_tklock_mode_change 'unlocked'";
            let cmd2 = "gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.req_display_state_on";
            pool.execute_command(device_host, cmd1, false).await?;
            pool.execute_command(device_host, cmd2, false).await?;
            info!("Screen unlocked via MCE D-Bus");
            Ok(vec!["Screen unlocked".to_string()])
        }

        _ => {
            let valid_keys = "power, home, back, volumeup/vol+, volumedown/vol-, menu, close, lock, unlock/wakeup";
            Err(anyhow!(
                "Unknown key: '{}'. Valid keys for Aurora OS: {}",
                key_name,
                valid_keys
            ))
        }
    }
}

/// Execute Screenshot command
async fn execute_screenshot(
    pool: &ConnectionPool,
    device_host: &str,
) -> Result<Vec<u8>> {
    info!("Taking screenshot on device {}", device_host);

    // Generate timestamped filename
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let remote_filename = format!("/home/defaultuser/Pictures/Screenshots/audb_screenshot_{}.png", timestamp);

    // Execute D-Bus screenshot command (needs root)
    let dbus_command = format!(
        "dbus-send --session --print-reply \
         --dest=org.nemomobile.lipstick \
         /org/nemomobile/lipstick/screenshot \
         org.nemomobile.lipstick.saveScreenshot \
         string:\"{}\"",
        remote_filename
    );

    pool.execute_command(device_host, &dbus_command, true).await?;

    // Read screenshot file as base64 (needs root)
    let read_command = format!("base64 {}", remote_filename);
    let base64_lines = pool.execute_command(device_host, &read_command, true).await?;
    let base64_data = base64_lines.join("").replace(['\n', '\r'], "");

    // Decode base64 to binary
    use base64::Engine;
    let binary_data = base64::engine::general_purpose::STANDARD.decode(&base64_data)
        .map_err(|e| anyhow!("Failed to decode base64 screenshot: {}", e))?;

    // Cleanup remote file
    let cleanup_cmd = format!("rm -f {}", remote_filename);
    pool.execute_command(device_host, &cleanup_cmd, true).await.ok();

    info!("Screenshot captured successfully ({} bytes)", binary_data.len());
    Ok(binary_data)
}

/// Execute Launch command
async fn execute_launch(
    pool: &ConnectionPool,
    device_host: &str,
    app_name: &str,
) -> Result<Vec<String>> {
    info!("Launching {} on device {}", app_name, device_host);

    // Validate app name
    if app_name.is_empty() {
        return Err(anyhow!("App name cannot be empty"));
    }
    if !app_name.contains('.') {
        return Err(anyhow!(
            "Invalid app name: '{}'. Expected D-Bus format: ru.domain.AppName",
            app_name
        ));
    }

    // Build D-Bus launch command
    let launch_command = format!(
        "gdbus call --system --dest ru.omp.RuntimeManager \
         --object-path /ru/omp/RuntimeManager/Control1 \
         --method ru.omp.RuntimeManager.Control1.Start \"{}\"",
        app_name
    );

    // Execute via pool (doesn't need root)
    let output = pool.execute_command(device_host, &launch_command, false).await?;

    info!("Application launched successfully");
    Ok(output)
}

/// Execute Stop command
async fn execute_stop(
    pool: &ConnectionPool,
    device_host: &str,
    app_name: &str,
) -> Result<Vec<String>> {
    info!("Stopping {} on device {}", app_name, device_host);

    // Validate app name
    if app_name.is_empty() {
        return Err(anyhow!("App name cannot be empty"));
    }
    if !app_name.contains('.') {
        return Err(anyhow!(
            "Invalid app name: '{}'. Expected D-Bus format: ru.domain.AppName",
            app_name
        ));
    }

    // Build D-Bus stop command
    let stop_command = format!(
        "gdbus call --system --dest ru.omp.RuntimeManager \
         --object-path /ru/omp/RuntimeManager/Control1 \
         --method ru.omp.RuntimeManager.Control1.Terminate \"{}\"",
        app_name
    );

    // Execute via pool (doesn't need root)
    let output = pool.execute_command(device_host, &stop_command, false).await?;

    info!("Application stopped successfully");
    Ok(output)
}

/// Execute Logs command
async fn execute_logs(
    pool: &ConnectionPool,
    device_host: &str,
    args: audb_protocol::LogsArgs,
) -> Result<Vec<String>> {
    info!("Retrieving logs from device {}", device_host);

    // Validate args
    if args.lines == 0 {
        return Err(anyhow!("Lines count must be greater than 0"));
    }
    if args.kernel && args.unit.is_some() {
        return Err(anyhow!("Cannot specify both --kernel and --unit"));
    }

    // Handle clear logs
    if args.clear {
        if !args.force {
            return Err(anyhow!("Clearing logs requires --force flag"));
        }
        let clear_command = "journalctl --rotate && journalctl --vacuum-time=1s";
        return pool.execute_command(device_host, clear_command, true).await;
    }

    // Build journalctl command
    let command = build_journalctl_command(&args)?;

    // Execute with root access
    let output = pool.execute_command(device_host, &command, true).await?;

    info!("Retrieved {} log lines", output.len());
    Ok(output)
}

/// Build journalctl command from args
fn build_journalctl_command(args: &audb_protocol::LogsArgs) -> Result<String> {
    let mut cmd = String::from("journalctl");

    // Kernel messages mode
    if args.kernel {
        cmd.push_str(" -k");
    }

    // Number of lines
    cmd.push_str(&format!(" -n {}", args.lines));

    // Priority level
    if let Some(ref priority) = args.priority {
        cmd.push_str(&format!(" -p {}", priority));
    }

    // Unit filter (with shell escaping)
    if let Some(ref unit) = args.unit {
        let escaped = escape_single_quote(unit);
        cmd.push_str(&format!(" -u '{}'", escaped));
    }

    // Time filter (with shell escaping)
    if let Some(ref since) = args.since {
        let escaped = escape_single_quote(since);
        cmd.push_str(&format!(" --since '{}'", escaped));
    }

    // Output options
    cmd.push_str(" --no-pager --no-hostname");

    // Grep filter (as pipe, with escaping)
    if let Some(ref grep_pattern) = args.grep {
        let escaped = escape_single_quote(grep_pattern);
        cmd.push_str(&format!(" | grep '{}'", escaped));
    }

    Ok(cmd)
}

/// Escape single quotes for shell command (simple implementation)
fn escape_single_quote(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Execute Uninstall command
async fn execute_uninstall(
    pool: &ConnectionPool,
    device_host: &str,
    package_name: &str,
) -> Result<Vec<String>> {
    info!("Uninstalling {} from device {}", package_name, device_host);

    // Validate package name
    if package_name.is_empty() {
        return Err(anyhow!("Package name cannot be empty"));
    }

    // Use APM D-Bus to remove package
    let uninstall_command = format!(
        "gdbus call --system --dest ru.omp.APM --object-path /ru/omp/APM --method ru.omp.APM.Remove \"{}\" \"{{}}\"",
        package_name
    );

    let output = pool.execute_command(device_host, &uninstall_command, false).await?;

    info!("Package uninstalled successfully");
    Ok(output)
}

/// Execute Packages command - list installed packages
async fn execute_packages(
    pool: &ConnectionPool,
    device_host: &str,
    filter: Option<String>,
) -> Result<Vec<String>> {
    info!("Listing packages on device {}", device_host);

    // Use APM D-Bus to get package list
    let list_command = "gdbus call --system --dest ru.omp.APM --object-path /ru/omp/APM --method ru.omp.APM.GetPackageList";

    let output = pool.execute_command(device_host, list_command, false).await?;

    // Parse the D-Bus output and extract package IDs
    // Output format: ([{'general.id': 'pkg1', ...}, ...],)
    let mut packages: Vec<String> = Vec::new();

    for line in &output {
        // Extract package IDs from the D-Bus response
        // Look for 'general.id': 'value' patterns
        let mut remaining = line.as_str();
        while let Some(start) = remaining.find("'general.id': '") {
            remaining = &remaining[start + 15..];
            if let Some(end) = remaining.find('\'') {
                let id = &remaining[..end];
                
                // Apply filter if specified
                if let Some(ref f) = filter {
                    if id.to_lowercase().contains(&f.to_lowercase()) {
                        packages.push(id.to_string());
                    }
                } else {
                    packages.push(id.to_string());
                }
                remaining = &remaining[end + 1..];
            } else {
                break;
            }
        }
    }

    // Sort packages alphabetically
    packages.sort();

    info!("Found {} packages", packages.len());
    Ok(packages)
}

/// Execute Push command - upload file to device
async fn execute_push(
    pool: &ConnectionPool,
    device_host: &str,
    local_path: &str,
    remote_path: &str,
    data: Vec<u8>,
) -> Result<Vec<String>> {
    info!("Pushing {} to {} on device {}", local_path, remote_path, device_host);

    // Get just the filename for temp file
    let file_name = std::path::Path::new(local_path)
        .file_name()
        .ok_or_else(|| anyhow!("Invalid local path"))?
        .to_string_lossy()
        .to_string();

    // Write data to temporary local file
    let local_temp = std::env::temp_dir().join(&file_name);
    std::fs::write(&local_temp, &data)?;

    // Upload to device
    let remote = PathBuf::from(remote_path);
    pool.upload_file(device_host, &local_temp, &remote).await?;

    // Cleanup local temp file
    std::fs::remove_file(&local_temp).ok();

    let size = data.len();
    info!("Pushed {} bytes to {}", size, remote_path);
    Ok(vec![format!("{}: {} bytes", remote_path, size)])
}

/// Execute Pull command - download file from device
async fn execute_pull(
    pool: &ConnectionPool,
    device_host: &str,
    remote_path: &str,
) -> Result<Vec<u8>> {
    info!("Pulling {} from device {}", remote_path, device_host);

    // Get filename for temp file
    let file_name = std::path::Path::new(remote_path)
        .file_name()
        .ok_or_else(|| anyhow!("Invalid remote path"))?
        .to_string_lossy()
        .to_string();

    // Download to temporary local file
    let local_temp = std::env::temp_dir().join(&file_name);
    let remote = PathBuf::from(remote_path);

    pool.download_file(device_host, &remote, &local_temp).await?;

    // Read file contents
    let data = std::fs::read(&local_temp)
        .map_err(|e| anyhow!("Failed to read downloaded file: {}", e))?;

    // Cleanup temp file
    std::fs::remove_file(&local_temp).ok();

    info!("Pulled {} bytes from {}", data.len(), remote_path);
    Ok(data)
}

/// Execute Info command - get device information
async fn execute_info(
    pool: &ConnectionPool,
    device_host: &str,
    _category: Option<String>,
) -> Result<audb_protocol::DeviceInfo> {
    info!("Getting device info from {}", device_host);

    // D-Bus calls to ru.omp.deviceinfo.Features (system bus)
    let dbus_base = "gdbus call --system --dest ru.omp.deviceinfo --object-path /ru/omp/deviceinfo/Features --method ru.omp.deviceinfo.Features";

    // Helper to extract value from D-Bus response like "('value',)" or "(uint32 123,)"
    let extract_string = |output: &[String]| -> String {
        output.first()
            .map(|s| {
                // Remove outer parens and trailing comma
                let s = s.trim_matches(|c| c == '(' || c == ')' || c == ',').trim();
                // Remove quotes if present
                s.trim_matches('\'').to_string()
            })
            .unwrap_or_default()
    };

    let extract_u32 = |output: &[String]| -> u32 {
        output.first()
            .and_then(|s| {
                // Handle format like "(uint32 8,)" or "(123,)"
                let s = s.trim_matches(|c| c == '(' || c == ')' || c == ',').trim();
                // Remove type prefix if present
                let s = if s.starts_with("uint32 ") {
                    &s[7..]
                } else {
                    s
                };
                s.parse().ok()
            })
            .unwrap_or(0)
    };

    let extract_u64 = |output: &[String]| -> u64 {
        output.first()
            .and_then(|s| {
                // Handle format like "(uint64 123456,)" or "(123456,)"
                let s = s.trim_matches(|c| c == '(' || c == ')' || c == ',').trim();
                // Remove type prefix if present
                let s = if s.starts_with("uint64 ") {
                    &s[7..]
                } else {
                    s
                };
                s.parse().ok()
            })
            .unwrap_or(0)
    };

    let extract_f64 = |output: &[String]| -> f64 {
        output.first()
            .and_then(|s| {
                let s = s.trim_matches(|c| c == '(' || c == ')' || c == ',').trim();
                s.parse().ok()
            })
            .unwrap_or(0.0)
    };

    let extract_bool = |output: &[String]| -> bool {
        output.first()
            .map(|s| s.contains("true"))
            .unwrap_or(false)
    };

    // Get device model
    let device_model = pool.execute_command(device_host, &format!("{}.getDeviceModel", dbus_base), false).await
        .map(|o| extract_string(&o))
        .unwrap_or_else(|_| "Unknown".to_string());

    // Get OS version
    let os_version = pool.execute_command(device_host, &format!("{}.getOsVersion", dbus_base), false).await
        .map(|o| extract_string(&o))
        .unwrap_or_else(|_| "Unknown".to_string());

    // Get screen resolution
    let screen_resolution = pool.execute_command(device_host, &format!("{}.getScreenResolution", dbus_base), false).await
        .map(|o| extract_string(&o))
        .unwrap_or_else(|_| "Unknown".to_string());

    // Get CPU model
    let cpu_model = pool.execute_command(device_host, &format!("{}.getCpuModel", dbus_base), false).await
        .map(|o| extract_string(&o))
        .unwrap_or_else(|_| "Unknown".to_string());

    // Get CPU cores
    let cpu_cores = pool.execute_command(device_host, &format!("{}.getNumberCpuCores", dbus_base), false).await
        .map(|o| extract_u32(&o))
        .unwrap_or(0);

    // Get CPU max clock
    let cpu_max_clock = pool.execute_command(device_host, &format!("{}.getMaxCpuClockSpeed", dbus_base), false).await
        .map(|o| extract_u32(&o))
        .unwrap_or(0);

    // Get RAM total (bytes -> MB)
    let ram_total_mb = pool.execute_command(device_host, &format!("{}.getRamTotalSize", dbus_base), false).await
        .map(|o| extract_u64(&o) / (1024 * 1024))
        .unwrap_or(0);

    // Get memory info from /proc/meminfo
    let meminfo = pool.execute_command(
        device_host,
        "awk '/MemAvailable/{a=$2} /MemFree/{f=$2} /^Buffers/{b=$2} /^Cached/{c=$2} END{print a,f,b,c}' /proc/meminfo",
        false
    ).await.unwrap_or_default();

    let mem_parts: Vec<u64> = meminfo.first()
        .map(|s| s.split_whitespace().filter_map(|p| p.parse().ok()).collect())
        .unwrap_or_default();

    let ram_available_mb = mem_parts.first().copied().unwrap_or(0) / 1024;
    let ram_free_mb = mem_parts.get(1).copied().unwrap_or(0) / 1024;
    let ram_buffers_mb = mem_parts.get(2).copied().unwrap_or(0) / 1024;
    let ram_cached_mb = mem_parts.get(3).copied().unwrap_or(0) / 1024;

    // Get battery level from com.nokia.mce
    let battery_level = pool.execute_command(
        device_host,
        "gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.get_battery_level",
        false
    ).await
        .map(|o| extract_u32(&o))
        .unwrap_or(0);

    // Get charger state from com.nokia.mce
    let charger_state = pool.execute_command(
        device_host,
        "gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.get_charger_state",
        false
    ).await
        .map(|o| extract_string(&o))
        .unwrap_or_else(|_| "unknown".to_string());

    let battery_state = if battery_level == 100 {
        "full".to_string()
    } else if charger_state == "on" {
        "charging".to_string()
    } else {
        "discharging".to_string()
    };

    // Get features (NFC, Bluetooth, WLAN, GNSS)
    let has_nfc = pool.execute_command(device_host, &format!("{}.hasNFC", dbus_base), false).await
        .map(|o| extract_bool(&o))
        .unwrap_or(false);

    let has_bluetooth = pool.execute_command(device_host, &format!("{}.hasBluetooth", dbus_base), false).await
        .map(|o| extract_bool(&o))
        .unwrap_or(false);

    let has_wlan = pool.execute_command(device_host, &format!("{}.hasWlan", dbus_base), false).await
        .map(|o| extract_bool(&o))
        .unwrap_or(false);

    let has_gnss = pool.execute_command(device_host, &format!("{}.hasGNSS", dbus_base), false).await
        .map(|o| extract_bool(&o))
        .unwrap_or(false);

    // Get camera resolutions
    let main_camera_mp = pool.execute_command(device_host, &format!("{}.getMainCameraResolution", dbus_base), false).await
        .map(|o| extract_f64(&o))
        .unwrap_or(0.0);

    let frontal_camera_mp = pool.execute_command(device_host, &format!("{}.getFrontalCameraResolution", dbus_base), false).await
        .map(|o| extract_f64(&o))
        .unwrap_or(0.0);

    // Get storage info using stat -f (more reliable than df)
    let storage_info = pool.execute_command(
        device_host,
        "stat -f -c '%b %a %S' /home",
        false
    ).await.unwrap_or_default();

    let storage_parts: Vec<u64> = storage_info.first()
        .map(|s| s.split_whitespace().filter_map(|p| p.parse().ok()).collect())
        .unwrap_or_default();

    let block_size = storage_parts.get(2).copied().unwrap_or(4096);
    let internal_storage_total_mb = storage_parts.first().copied().unwrap_or(0) * block_size / (1024 * 1024);
    let internal_storage_free_mb = storage_parts.get(1).copied().unwrap_or(0) * block_size / (1024 * 1024);

    info!("Device info retrieved successfully");

    Ok(audb_protocol::DeviceInfo {
        device_model,
        os_version,
        screen_resolution,
        cpu_model,
        cpu_cores,
        cpu_max_clock,
        ram_total_mb,
        ram_available_mb,
        ram_free_mb,
        ram_cached_mb,
        ram_buffers_mb,
        battery_level,
        battery_state,
        has_nfc,
        has_bluetooth,
        has_wlan,
        has_gnss,
        main_camera_mp,
        frontal_camera_mp,
        internal_storage_total_mb,
        internal_storage_free_mb,
    })
}

/// Execute Open command - open URL on device
async fn execute_open(
    pool: &ConnectionPool,
    device_host: &str,
    url: &str,
) -> Result<Vec<String>> {
    info!("Opening URL '{}' on device {}", url, device_host);

    // Use sailfish fileservice D-Bus to open URL
    let dbus_command = format!(
        "gdbus call --session --dest org.sailfishos.fileservice --object-path / --method org.sailfishos.fileservice.openUrl '{}'",
        url.replace('\'', "'\\''")  // Escape single quotes
    );

    pool.execute_command(device_host, &dbus_command, false).await?;

    info!("URL opened successfully");
    Ok(vec![format!("Opened: {}", url)])
}
