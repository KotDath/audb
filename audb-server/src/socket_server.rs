use crate::emulator::EmulatorManager;
use crate::pool::ConnectionPool;
use anyhow::{anyhow, Result};
use audb_core::features::config::device_store::DeviceStore;
use audb_core::tools::types::{Device, DeviceIdentifier, DeviceKind};
use audb_protocol::{
    recv_message, send_message, Command, CommandOutput, CommandResult, Request, Response,
    ServerStatus,
};
use nix::unistd::Uid;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info};

const BRIDGE_SERVICE: &str = "ru.kotdath.AudbBridge";
const BRIDGE_OBJECT_PATH: &str = "/ru/kotdath/AudbBridge";
const BRIDGE_INTERFACE: &str = "ru.kotdath.AudbBridge";
const BRIDGE_SESSION_ENV: &str =
    "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/dbus/user_bus_socket";

/// Get the path to the Unix socket
pub fn socket_path() -> PathBuf {
    let uid = Uid::current();
    PathBuf::from(format!("/tmp/audb-server-{}.sock", uid))
}

/// Start the Unix socket server
pub async fn start_server(
    pool: Arc<ConnectionPool>,
    emulator_manager: Arc<EmulatorManager>,
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
                        let emulator_manager_clone = Arc::clone(&emulator_manager);
                        tokio::spawn(async move {
                            if let Err(e) = handle_client(stream, pool_clone, emulator_manager_clone).await {
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
async fn handle_client(
    mut stream: UnixStream,
    pool: Arc<ConnectionPool>,
    emulator_manager: Arc<EmulatorManager>,
) -> Result<()> {
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
        let result = process_command(request.command, &pool, &emulator_manager).await;

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
async fn process_command(
    command: Command,
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
) -> CommandResult {
    match command {
        Command::Ping => {
            // Simple ping/pong for testing
            CommandResult::Success {
                output: CommandOutput::Lines(vec!["pong".to_string()]),
            }
        }

        Command::ServerStatus => {
            // Return server status
            match get_server_status(pool, emulator_manager).await {
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
        Command::Shell {
            device,
            root,
            command,
        } => match execute_shell(pool, emulator_manager, &device, &command, root).await {
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
        },

        Command::Install {
            device,
            rpm_path,
            rpm_data,
        } => match execute_install(pool, emulator_manager, &device, &rpm_path, rpm_data).await {
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
        },

        Command::Uninstall {
            device,
            package_name,
        } => match execute_uninstall(pool, emulator_manager, &device, &package_name).await {
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
        },

        Command::Packages { device, filter } => {
            match execute_packages(pool, emulator_manager, &device, filter).await {
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

        Command::Push {
            device,
            local_path,
            remote_path,
            data,
        } => {
            match execute_push(
                pool,
                emulator_manager,
                &device,
                &local_path,
                &remote_path,
                data,
            )
            .await
            {
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

        Command::Pull {
            device,
            remote_path,
        } => match execute_pull(pool, emulator_manager, &device, &remote_path).await {
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
        },

        Command::Info { device, category } => {
            match execute_info(pool, emulator_manager, &device, category).await {
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

        Command::Tap {
            device,
            x,
            y,
            event_device,
            duration_ms,
        } => {
            match execute_tap(
                pool,
                emulator_manager,
                &device,
                x,
                y,
                event_device,
                duration_ms,
            )
            .await
            {
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

        Command::Swipe {
            device,
            mode,
            event_device,
            steps,
            duration_ms,
            hold_ms,
        } => {
            match execute_swipe(
                pool,
                emulator_manager,
                &device,
                mode,
                event_device,
                steps,
                duration_ms,
                hold_ms,
            )
            .await
            {
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
            match execute_key(pool, emulator_manager, &device, &key_name).await {
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
            match execute_screenshot(pool, emulator_manager, &device).await {
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
            match execute_launch(pool, emulator_manager, &device, &app_name).await {
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
            match execute_stop(pool, emulator_manager, &device, &app_name).await {
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
            match execute_logs(pool, emulator_manager, &device, args).await {
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
            match execute_reconnect(pool, emulator_manager, device).await {
                Ok(output) => CommandResult::Success {
                    output: CommandOutput::Lines(output),
                },
                Err(e) => CommandResult::Error {
                    message: e.to_string(),
                    kind: audb_protocol::ErrorKind::ServerError,
                },
            }
        }

        Command::EmulatorStart { device } => {
            match execute_emulator_start(pool, emulator_manager, device).await {
                Ok(status) => CommandResult::Success {
                    output: CommandOutput::EmulatorStatus(status),
                },
                Err(e) => CommandResult::Error {
                    message: e.to_string(),
                    kind: audb_protocol::ErrorKind::CommandFailed,
                },
            }
        }

        Command::EmulatorStop { device } => {
            match execute_emulator_stop(pool, emulator_manager, device).await {
                Ok(status) => CommandResult::Success {
                    output: CommandOutput::EmulatorStatus(status),
                },
                Err(e) => CommandResult::Error {
                    message: e.to_string(),
                    kind: audb_protocol::ErrorKind::CommandFailed,
                },
            }
        }

        Command::EmulatorStatus { device } => {
            match execute_emulator_status(pool, emulator_manager, device).await {
                Ok(status) => CommandResult::Success {
                    output: CommandOutput::EmulatorStatus(status),
                },
                Err(e) => CommandResult::Error {
                    message: e.to_string(),
                    kind: audb_protocol::ErrorKind::CommandFailed,
                },
            }
        }

        Command::Open { device, url } => {
            match execute_open(pool, emulator_manager, &device, &url).await {
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
async fn resolve_device(pool: &ConnectionPool, device_ref: &str) -> Result<Device> {
    let identifier = DeviceIdentifier::parse(device_ref);
    let device = DeviceStore::find(&identifier)?;
    if !device.enabled {
        return Err(anyhow!("Device {} is disabled", device.display_name()));
    }
    pool.ensure_device(device.clone()).await;
    Ok(device)
}

async fn resolve_optional_emulator_device(
    pool: &ConnectionPool,
    device_ref: Option<String>,
) -> Result<Device> {
    let device_ref = if let Some(device_ref) = device_ref {
        device_ref
    } else {
        let current = audb_core::features::config::state::DeviceState::get_current()?;
        current
    };
    let device = resolve_device(pool, &device_ref).await?;
    if device.kind != DeviceKind::QemuEmulator {
        return Err(anyhow!(
            "Selected device '{}' is not an emulator",
            device.display_name()
        ));
    }
    Ok(device)
}

async fn ensure_guest_device(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
) -> Result<Device> {
    let device = resolve_device(pool, device_ref).await?;
    if device.kind == DeviceKind::QemuEmulator {
        emulator_manager.ensure_runtime(&device, pool).await?;
    }
    Ok(device)
}

async fn get_server_status(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
) -> Result<ServerStatus> {
    use crate::connection::ConnectionState;
    use audb_protocol::{ConnectionStateInfo, DeviceStatus};

    let pid = std::process::id();
    let socket_path = socket_path();

    let mut device_statuses: Vec<DeviceStatus> = vec![];

    for device in DeviceStore::list_enabled()? {
        pool.ensure_device(device.clone()).await;
        if let Ok(conn) = pool.get_device_info(&device.id).await {
            let state_info = match conn.state.clone() {
                ConnectionState::Disconnected => ConnectionStateInfo::Disconnected,
                ConnectionState::Connecting { attempt, .. } => {
                    ConnectionStateInfo::Connecting { attempt }
                }
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
                id: conn.device.id.clone(),
                name: conn.device.name.clone(),
                host: conn.device.host.clone(),
                port: conn.device.port,
                arch: match conn.device.arch {
                    audb_core::tools::types::DeviceArch::AuroraArm => {
                        audb_protocol::DeviceArchInfo::AuroraArm
                    }
                    audb_core::tools::types::DeviceArch::AuroraArm64 => {
                        audb_protocol::DeviceArchInfo::AuroraArm64
                    }
                    audb_core::tools::types::DeviceArch::AuroraX86_64 => {
                        audb_protocol::DeviceArchInfo::AuroraX86_64
                    }
                },
                kind: match conn.device.kind {
                    DeviceKind::Physical => audb_protocol::DeviceKindInfo::Physical,
                    DeviceKind::QemuEmulator => audb_protocol::DeviceKindInfo::QemuEmulator,
                },
                state: state_info,
                stats: audb_protocol::ConnectionStats {
                    connect_attempts: conn.stats.connect_attempts,
                    successful_commands: conn.stats.successful_commands,
                    failed_commands: conn.stats.failed_commands,
                    last_error: conn.stats.last_error.clone(),
                },
                emulator: if conn.device.kind == DeviceKind::QemuEmulator {
                    Some(emulator_manager.status(&conn.device, pool).await?)
                } else {
                    None
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

async fn execute_shell(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    command: &str,
    as_root: bool,
) -> Result<Vec<String>> {
    let device = ensure_guest_device(pool, emulator_manager, device_ref).await?;
    pool.execute_command(&device.id, command, as_root).await
}

/// Execute Install command
async fn execute_install(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    rpm_path: &str,
    rpm_data: Vec<u8>,
) -> Result<Vec<String>> {
    let device = ensure_guest_device(pool, emulator_manager, device_ref).await?;
    info!(
        "Installing {} on device {}",
        rpm_path,
        device.display_name()
    );

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
    pool.upload_file(&device.id, &local_temp, &remote_path)
        .await?;

    // Cleanup local temp file
    std::fs::remove_file(&local_temp).ok();

    // Install via D-Bus APM
    info!("Installing package via APM...");
    let install_command = format!(
        "gdbus call --system --dest ru.omp.APM --object-path /ru/omp/APM --method ru.omp.APM.Install \"{}\" \"{{}}\"",
        remote_path.display()
    );

    let output = pool
        .execute_command(&device.id, &install_command, false)
        .await?;

    // Cleanup remote file
    let cleanup_command = format!("rm -f {}", remote_path.display());
    pool.execute_command(&device.id, &cleanup_command, false)
        .await
        .ok();

    info!("Package installed successfully");
    Ok(output)
}

/// Execute Tap command
async fn execute_tap(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    x: u16,
    y: u16,
    event_device: Option<String>,
    duration_ms: Option<u32>,
) -> Result<Vec<String>> {
    let device = resolve_device(pool, device_ref).await?;
    info!(
        "Tapping at ({}, {}) on device {}",
        x,
        y,
        device.display_name()
    );

    // Validate coordinates
    if x > 4096 || y > 4096 {
        return Err(anyhow!(
            "Coordinates out of range: ({}, {}). Max: 4096x4096",
            x,
            y
        ));
    }

    if device.kind == DeviceKind::QemuEmulator {
        if event_device.is_some() {
            return Err(anyhow!("--event is unsupported for qemu-emulator backend"));
        }
        emulator_manager.tap(&device, pool, x, y, duration_ms).await
    } else {
        let tap_command = build_bridge_tap_command(x, y, event_device, duration_ms);
        info!("Executing tap via AudbBridge D-Bus...");
        pool.execute_command(&device.id, &tap_command, false)
            .await?;

        Ok(vec![format!("tap({}, {}) via AudbBridge", x, y)])
    }
}

/// Execute Swipe command
async fn execute_swipe(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    mode: audb_protocol::SwipeMode,
    event_device: Option<String>,
    steps: Option<u32>,
    duration_ms: Option<u32>,
    hold_ms: Option<u32>,
) -> Result<Vec<String>> {
    let device = resolve_device(pool, device_ref).await?;
    info!("Executing swipe on device {}", device.display_name());

    // Validate coordinates if needed
    if let audb_protocol::SwipeMode::Coords { x1, y1, x2, y2 } = &mode {
        for coord in [x1, y1, x2, y2] {
            if *coord > 4096 {
                return Err(anyhow!("Coordinate out of range: {}. Max: 4096", coord));
            }
        }
    }

    if device.kind == DeviceKind::QemuEmulator {
        if event_device.is_some() {
            return Err(anyhow!("--event is unsupported for qemu-emulator backend"));
        }
        emulator_manager
            .swipe(&device, pool, mode, steps, duration_ms, hold_ms)
            .await
    } else {
        let swipe_command = build_bridge_swipe_command(mode, event_device);
        info!("Executing swipe via AudbBridge D-Bus...");
        pool.execute_command(&device.id, &swipe_command, false)
            .await?;

        Ok(vec!["swipe via AudbBridge".to_string()])
    }
}

/// Execute Key command
async fn execute_key(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    key_name: &str,
) -> Result<Vec<String>> {
    let device = resolve_device(pool, device_ref).await?;
    info!(
        "Sending key '{}' on device {}",
        key_name,
        device.display_name()
    );

    if device.kind == DeviceKind::QemuEmulator {
        emulator_manager.key(&device, pool, key_name).await
    } else {
        let key_command = build_bridge_key_command(key_name);
        info!("Executing key via AudbBridge D-Bus...");
        pool.execute_command(&device.id, &key_command, false)
            .await?;

        Ok(vec![format!("key '{}' via AudbBridge", key_name)])
    }
}

/// Execute Screenshot command
async fn execute_screenshot(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
) -> Result<Vec<u8>> {
    let device = resolve_device(pool, device_ref).await?;
    info!("Taking screenshot on device {}", device.display_name());

    if device.kind == DeviceKind::QemuEmulator {
        return emulator_manager.screenshot(&device, pool).await;
    }

    // Generate timestamped filename
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let remote_filename = format!(
        "/home/defaultuser/Pictures/Screenshots/audb_screenshot_{}.png",
        timestamp
    );

    // Execute D-Bus screenshot command (needs root)
    let dbus_command = format!(
        "dbus-send --session --print-reply \
         --dest=org.nemomobile.lipstick \
         /org/nemomobile/lipstick/screenshot \
         org.nemomobile.lipstick.saveScreenshot \
         string:\"{}\"",
        remote_filename
    );

    pool.execute_command(&device.id, &dbus_command, true)
        .await?;

    // Read screenshot file as base64 (needs root)
    let read_command = format!("base64 {}", remote_filename);
    let base64_lines = pool
        .execute_command(&device.id, &read_command, true)
        .await?;
    let base64_data = base64_lines.join("").replace(['\n', '\r'], "");

    // Decode base64 to binary
    use base64::Engine;
    let binary_data = base64::engine::general_purpose::STANDARD
        .decode(&base64_data)
        .map_err(|e| anyhow!("Failed to decode base64 screenshot: {}", e))?;

    // Cleanup remote file
    let cleanup_cmd = format!("rm -f {}", remote_filename);
    pool.execute_command(&device.id, &cleanup_cmd, true)
        .await
        .ok();

    info!(
        "Screenshot captured successfully ({} bytes)",
        binary_data.len()
    );
    Ok(binary_data)
}

/// Execute Launch command
async fn execute_launch(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    app_name: &str,
) -> Result<Vec<String>> {
    let device = ensure_guest_device(pool, emulator_manager, device_ref).await?;
    info!("Launching {} on device {}", app_name, device.display_name());

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
    let output = pool
        .execute_command(&device.id, &launch_command, false)
        .await?;

    info!("Application launched successfully");
    Ok(output)
}

/// Execute Stop command
async fn execute_stop(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    app_name: &str,
) -> Result<Vec<String>> {
    let device = ensure_guest_device(pool, emulator_manager, device_ref).await?;
    info!("Stopping {} on device {}", app_name, device.display_name());

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
    let output = pool
        .execute_command(&device.id, &stop_command, false)
        .await?;

    info!("Application stopped successfully");
    Ok(output)
}

/// Execute Logs command
async fn execute_logs(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    args: audb_protocol::LogsArgs,
) -> Result<Vec<String>> {
    let device = ensure_guest_device(pool, emulator_manager, device_ref).await?;
    info!("Retrieving logs from device {}", device.display_name());

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
        return pool.execute_command(&device.id, clear_command, true).await;
    }

    // Build journalctl command
    let command = build_journalctl_command(&args)?;

    // Execute with root access
    let output = pool.execute_command(&device.id, &command, true).await?;

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

fn build_bridge_tap_command(
    x: u16,
    y: u16,
    event_device: Option<String>,
    duration_ms: Option<u32>,
) -> String {
    let options = build_tap_options_map(event_device, duration_ms);
    build_bridge_command("Tap", &[x.to_string(), y.to_string(), options])
}

fn build_bridge_swipe_command(
    mode: audb_protocol::SwipeMode,
    event_device: Option<String>,
) -> String {
    let options = build_swipe_options_map(event_device);

    match mode {
        audb_protocol::SwipeMode::Coords { x1, y1, x2, y2 } => build_bridge_command(
            "Swipe",
            &[
                x1.to_string(),
                y1.to_string(),
                x2.to_string(),
                y2.to_string(),
                options,
            ],
        ),
        audb_protocol::SwipeMode::Direction(dir) => build_bridge_command(
            "SwipeDirection",
            &[swipe_direction_to_bridge_arg(dir).to_string(), options],
        ),
    }
}

fn build_bridge_key_command(key_name: &str) -> String {
    build_bridge_command("Key", &[key_name.to_string()])
}

fn build_bridge_command(method: &str, arguments: &[String]) -> String {
    let mut parts = vec![
        BRIDGE_SESSION_ENV.to_string(),
        "gdbus".to_string(),
        "call".to_string(),
        "--session".to_string(),
        "--dest".to_string(),
        shell_quote(BRIDGE_SERVICE),
        "--object-path".to_string(),
        shell_quote(BRIDGE_OBJECT_PATH),
        "--method".to_string(),
        shell_quote(&format!("{}.{}", BRIDGE_INTERFACE, method)),
    ];

    parts.extend(arguments.iter().map(|argument| shell_quote(argument)));
    parts.join(" ")
}

fn build_tap_options_map(event_device: Option<String>, duration_ms: Option<u32>) -> String {
    let mut entries = Vec::new();

    if let Some(event_device) = event_device {
        entries.push(("eventDevice", gvariant_string(&event_device)));
    }
    if let Some(duration_ms) = duration_ms {
        entries.push(("durationMs", duration_ms.to_string()));
    }

    build_gvariant_dict(entries)
}

fn build_swipe_options_map(event_device: Option<String>) -> String {
    let mut entries = Vec::new();
    if let Some(event_device) = event_device {
        entries.push(("eventDevice", gvariant_string(&event_device)));
    }
    build_gvariant_dict(entries)
}

fn build_gvariant_dict(entries: Vec<(&str, String)>) -> String {
    if entries.is_empty() {
        return "{}".to_string();
    }

    let items = entries
        .into_iter()
        .map(|(key, value)| format!("'{}': <{}>", key, value))
        .collect::<Vec<_>>();
    format!("{{{}}}", items.join(", "))
}

fn gvariant_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn swipe_direction_to_bridge_arg(direction: audb_protocol::SwipeDirection) -> &'static str {
    match direction {
        audb_protocol::SwipeDirection::Left => "rl",
        audb_protocol::SwipeDirection::Right => "lr",
        audb_protocol::SwipeDirection::Up => "du",
        audb_protocol::SwipeDirection::Down => "ud",
        audb_protocol::SwipeDirection::LongUp => "longdu",
        audb_protocol::SwipeDirection::LongDown => "longud",
    }
}

/// Escape single quotes for shell command (simple implementation)
fn escape_single_quote(s: &str) -> String {
    s.replace('\'', "'\\''")
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", escape_single_quote(s))
}

/// Execute Uninstall command
async fn execute_uninstall(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    package_name: &str,
) -> Result<Vec<String>> {
    let device = ensure_guest_device(pool, emulator_manager, device_ref).await?;
    info!(
        "Uninstalling {} from device {}",
        package_name,
        device.display_name()
    );

    // Validate package name
    if package_name.is_empty() {
        return Err(anyhow!("Package name cannot be empty"));
    }

    // Use APM D-Bus to remove package
    let uninstall_command = format!(
        "gdbus call --system --dest ru.omp.APM --object-path /ru/omp/APM --method ru.omp.APM.Remove \"{}\" \"{{}}\"",
        package_name
    );

    let output = pool
        .execute_command(&device.id, &uninstall_command, false)
        .await?;

    info!("Package uninstalled successfully");
    Ok(output)
}

/// Execute Packages command - list installed packages
async fn execute_packages(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    filter: Option<String>,
) -> Result<Vec<String>> {
    let device = ensure_guest_device(pool, emulator_manager, device_ref).await?;
    info!("Listing packages on device {}", device.display_name());

    // Use APM D-Bus to get package list
    let list_command = "gdbus call --system --dest ru.omp.APM --object-path /ru/omp/APM --method ru.omp.APM.GetPackageList";

    let output = pool
        .execute_command(&device.id, list_command, false)
        .await?;

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
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    local_path: &str,
    remote_path: &str,
    data: Vec<u8>,
) -> Result<Vec<String>> {
    let device = ensure_guest_device(pool, emulator_manager, device_ref).await?;
    info!(
        "Pushing {} to {} on device {}",
        local_path,
        remote_path,
        device.display_name()
    );

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
    pool.upload_file(&device.id, &local_temp, &remote).await?;

    // Cleanup local temp file
    std::fs::remove_file(&local_temp).ok();

    let size = data.len();
    info!("Pushed {} bytes to {}", size, remote_path);
    Ok(vec![format!("{}: {} bytes", remote_path, size)])
}

/// Execute Pull command - download file from device
async fn execute_pull(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    remote_path: &str,
) -> Result<Vec<u8>> {
    let device = ensure_guest_device(pool, emulator_manager, device_ref).await?;
    info!(
        "Pulling {} from device {}",
        remote_path,
        device.display_name()
    );

    // Get filename for temp file
    let file_name = std::path::Path::new(remote_path)
        .file_name()
        .ok_or_else(|| anyhow!("Invalid remote path"))?
        .to_string_lossy()
        .to_string();

    // Download to temporary local file
    let local_temp = std::env::temp_dir().join(&file_name);
    let remote = PathBuf::from(remote_path);

    pool.download_file(&device.id, &remote, &local_temp).await?;

    // Read file contents
    let data =
        std::fs::read(&local_temp).map_err(|e| anyhow!("Failed to read downloaded file: {}", e))?;

    // Cleanup temp file
    std::fs::remove_file(&local_temp).ok();

    info!("Pulled {} bytes from {}", data.len(), remote_path);
    Ok(data)
}

/// Execute Info command - get device information
async fn execute_info(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    _category: Option<String>,
) -> Result<audb_protocol::DeviceInfo> {
    let device = ensure_guest_device(pool, emulator_manager, device_ref).await?;
    info!("Getting device info from {}", device.display_name());

    // D-Bus calls to ru.omp.deviceinfo.Features (system bus)
    let dbus_base = "gdbus call --system --dest ru.omp.deviceinfo --object-path /ru/omp/deviceinfo/Features --method ru.omp.deviceinfo.Features";

    // Helper to extract value from D-Bus response like "('value',)" or "(uint32 123,)"
    let extract_string = |output: &[String]| -> String {
        output
            .first()
            .map(|s| {
                // Remove outer parens and trailing comma
                let s = s.trim_matches(|c| c == '(' || c == ')' || c == ',').trim();
                // Remove quotes if present
                s.trim_matches('\'').to_string()
            })
            .unwrap_or_default()
    };

    let extract_u32 = |output: &[String]| -> u32 {
        output
            .first()
            .and_then(|s| {
                // Handle format like "(uint32 8,)" or "(123,)"
                let s = s.trim_matches(|c| c == '(' || c == ')' || c == ',').trim();
                // Remove type prefix if present
                let s = if s.starts_with("uint32 ") { &s[7..] } else { s };
                s.parse().ok()
            })
            .unwrap_or(0)
    };

    let extract_u64 = |output: &[String]| -> u64 {
        output
            .first()
            .and_then(|s| {
                // Handle format like "(uint64 123456,)" or "(123456,)"
                let s = s.trim_matches(|c| c == '(' || c == ')' || c == ',').trim();
                // Remove type prefix if present
                let s = if s.starts_with("uint64 ") { &s[7..] } else { s };
                s.parse().ok()
            })
            .unwrap_or(0)
    };

    let extract_f64 = |output: &[String]| -> f64 {
        output
            .first()
            .and_then(|s| {
                let s = s.trim_matches(|c| c == '(' || c == ')' || c == ',').trim();
                s.parse().ok()
            })
            .unwrap_or(0.0)
    };

    let extract_bool =
        |output: &[String]| -> bool { output.first().map(|s| s.contains("true")).unwrap_or(false) };

    // Get device model
    let device_model = pool
        .execute_command(&device.id, &format!("{}.getDeviceModel", dbus_base), false)
        .await
        .map(|o| extract_string(&o))
        .unwrap_or_else(|_| "Unknown".to_string());

    // Get OS version
    let os_version = pool
        .execute_command(&device.id, &format!("{}.getOsVersion", dbus_base), false)
        .await
        .map(|o| extract_string(&o))
        .unwrap_or_else(|_| "Unknown".to_string());

    // Get screen resolution
    let screen_resolution = pool
        .execute_command(
            &device.id,
            &format!("{}.getScreenResolution", dbus_base),
            false,
        )
        .await
        .map(|o| extract_string(&o))
        .unwrap_or_else(|_| "Unknown".to_string());

    // Get CPU model
    let cpu_model = pool
        .execute_command(&device.id, &format!("{}.getCpuModel", dbus_base), false)
        .await
        .map(|o| extract_string(&o))
        .unwrap_or_else(|_| "Unknown".to_string());

    // Get CPU cores
    let cpu_cores = pool
        .execute_command(
            &device.id,
            &format!("{}.getNumberCpuCores", dbus_base),
            false,
        )
        .await
        .map(|o| extract_u32(&o))
        .unwrap_or(0);

    // Get CPU max clock
    let cpu_max_clock = pool
        .execute_command(
            &device.id,
            &format!("{}.getMaxCpuClockSpeed", dbus_base),
            false,
        )
        .await
        .map(|o| extract_u32(&o))
        .unwrap_or(0);

    // Get RAM total (bytes -> MB)
    let ram_total_mb = pool
        .execute_command(&device.id, &format!("{}.getRamTotalSize", dbus_base), false)
        .await
        .map(|o| extract_u64(&o) / (1024 * 1024))
        .unwrap_or(0);

    // Get memory info from /proc/meminfo
    let meminfo = pool.execute_command(
        &device.id,
        "awk '/MemAvailable/{a=$2} /MemFree/{f=$2} /^Buffers/{b=$2} /^Cached/{c=$2} END{print a,f,b,c}' /proc/meminfo",
        false
    ).await.unwrap_or_default();

    let mem_parts: Vec<u64> = meminfo
        .first()
        .map(|s| {
            s.split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect()
        })
        .unwrap_or_default();

    let ram_available_mb = mem_parts.first().copied().unwrap_or(0) / 1024;
    let ram_free_mb = mem_parts.get(1).copied().unwrap_or(0) / 1024;
    let ram_buffers_mb = mem_parts.get(2).copied().unwrap_or(0) / 1024;
    let ram_cached_mb = mem_parts.get(3).copied().unwrap_or(0) / 1024;

    // Get battery level from com.nokia.mce
    let battery_level = pool.execute_command(
        &device.id,
        "gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.get_battery_level",
        false
    ).await
        .map(|o| extract_u32(&o))
        .unwrap_or(0);

    // Get charger state from com.nokia.mce
    let charger_state = pool.execute_command(
        &device.id,
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
    let has_nfc = pool
        .execute_command(&device.id, &format!("{}.hasNFC", dbus_base), false)
        .await
        .map(|o| extract_bool(&o))
        .unwrap_or(false);

    let has_bluetooth = pool
        .execute_command(&device.id, &format!("{}.hasBluetooth", dbus_base), false)
        .await
        .map(|o| extract_bool(&o))
        .unwrap_or(false);

    let has_wlan = pool
        .execute_command(&device.id, &format!("{}.hasWlan", dbus_base), false)
        .await
        .map(|o| extract_bool(&o))
        .unwrap_or(false);

    let has_gnss = pool
        .execute_command(&device.id, &format!("{}.hasGNSS", dbus_base), false)
        .await
        .map(|o| extract_bool(&o))
        .unwrap_or(false);

    // Get camera resolutions
    let main_camera_mp = pool
        .execute_command(
            &device.id,
            &format!("{}.getMainCameraResolution", dbus_base),
            false,
        )
        .await
        .map(|o| extract_f64(&o))
        .unwrap_or(0.0);

    let frontal_camera_mp = pool
        .execute_command(
            &device.id,
            &format!("{}.getFrontalCameraResolution", dbus_base),
            false,
        )
        .await
        .map(|o| extract_f64(&o))
        .unwrap_or(0.0);

    // Get storage info using stat -f (more reliable than df)
    let storage_info = pool
        .execute_command(&device.id, "stat -f -c '%b %a %S' /home", false)
        .await
        .unwrap_or_default();

    let storage_parts: Vec<u64> = storage_info
        .first()
        .map(|s| {
            s.split_whitespace()
                .filter_map(|p| p.parse().ok())
                .collect()
        })
        .unwrap_or_default();

    let block_size = storage_parts.get(2).copied().unwrap_or(4096);
    let internal_storage_total_mb =
        storage_parts.first().copied().unwrap_or(0) * block_size / (1024 * 1024);
    let internal_storage_free_mb =
        storage_parts.get(1).copied().unwrap_or(0) * block_size / (1024 * 1024);

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
    emulator_manager: &EmulatorManager,
    device_ref: &str,
    url: &str,
) -> Result<Vec<String>> {
    let device = ensure_guest_device(pool, emulator_manager, device_ref).await?;
    info!("Opening URL '{}' on device {}", url, device.display_name());

    // Use sailfish fileservice D-Bus to open URL
    let dbus_command = format!(
        "gdbus call --session --dest org.sailfishos.fileservice --object-path / --method org.sailfishos.fileservice.openUrl '{}'",
        url.replace('\'', "'\\''")  // Escape single quotes
    );

    pool.execute_command(&device.id, &dbus_command, false)
        .await?;

    info!("URL opened successfully");
    Ok(vec![format!("Opened: {}", url)])
}

async fn execute_reconnect(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: Option<String>,
) -> Result<Vec<String>> {
    if let Some(device_ref) = device_ref {
        let device = resolve_device(pool, &device_ref).await?;
        pool.reset_device(&device.id).await.ok();
        if device.kind == DeviceKind::QemuEmulator {
            emulator_manager.reconnect(&device).await;
        }
        return Ok(vec![format!("reconnected {}", device.display_name())]);
    }

    for device in DeviceStore::list_enabled()? {
        pool.ensure_device(device.clone()).await;
        pool.reset_device(&device.id).await.ok();
        if device.kind == DeviceKind::QemuEmulator {
            emulator_manager.reconnect(&device).await;
        }
    }

    Ok(vec!["reconnected all devices".to_string()])
}

async fn execute_emulator_start(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: Option<String>,
) -> Result<audb_protocol::EmulatorStatus> {
    let device = resolve_optional_emulator_device(pool, device_ref).await?;
    emulator_manager.start(&device, pool).await
}

async fn execute_emulator_stop(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: Option<String>,
) -> Result<audb_protocol::EmulatorStatus> {
    let device = resolve_optional_emulator_device(pool, device_ref).await?;
    emulator_manager.stop(&device).await
}

async fn execute_emulator_status(
    pool: &ConnectionPool,
    emulator_manager: &EmulatorManager,
    device_ref: Option<String>,
) -> Result<audb_protocol::EmulatorStatus> {
    let device = resolve_optional_emulator_device(pool, device_ref).await?;
    emulator_manager.status(&device, pool).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_bridge_tap_command_without_options() {
        let command = build_bridge_tap_command(600, 1000, None, None);

        assert_eq!(
            command,
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/dbus/user_bus_socket gdbus call --session --dest 'ru.kotdath.AudbBridge' --object-path '/ru/kotdath/AudbBridge' --method 'ru.kotdath.AudbBridge.Tap' '600' '1000' '{}'"
        );
    }

    #[test]
    fn build_bridge_tap_command_with_options() {
        let command = build_bridge_tap_command(600, 1000, Some("auto".to_string()), Some(120));

        assert_eq!(
            command,
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/dbus/user_bus_socket gdbus call --session --dest 'ru.kotdath.AudbBridge' --object-path '/ru/kotdath/AudbBridge' --method 'ru.kotdath.AudbBridge.Tap' '600' '1000' '{'\\''eventDevice'\\'': <'\\''auto'\\''>, '\\''durationMs'\\'': <120>}'"
        );
    }

    #[test]
    fn build_bridge_swipe_direction_command() {
        let command = build_bridge_swipe_command(
            audb_protocol::SwipeMode::Direction(audb_protocol::SwipeDirection::Up),
            None,
        );

        assert_eq!(
            command,
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/dbus/user_bus_socket gdbus call --session --dest 'ru.kotdath.AudbBridge' --object-path '/ru/kotdath/AudbBridge' --method 'ru.kotdath.AudbBridge.SwipeDirection' 'du' '{}'"
        );
    }

    #[test]
    fn build_bridge_long_swipe_direction_command() {
        let command = build_bridge_swipe_command(
            audb_protocol::SwipeMode::Direction(audb_protocol::SwipeDirection::LongUp),
            None,
        );

        assert_eq!(
            command,
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/dbus/user_bus_socket gdbus call --session --dest 'ru.kotdath.AudbBridge' --object-path '/ru/kotdath/AudbBridge' --method 'ru.kotdath.AudbBridge.SwipeDirection' 'longdu' '{}'"
        );
    }

    #[test]
    fn build_bridge_swipe_coords_command_with_options() {
        let command = build_bridge_swipe_command(
            audb_protocol::SwipeMode::Coords {
                x1: 10,
                y1: 20,
                x2: 30,
                y2: 40,
            },
            Some("/dev/input/event4".to_string()),
        );

        assert_eq!(
            command,
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/dbus/user_bus_socket gdbus call --session --dest 'ru.kotdath.AudbBridge' --object-path '/ru/kotdath/AudbBridge' --method 'ru.kotdath.AudbBridge.Swipe' '10' '20' '30' '40' '{'\\''eventDevice'\\'': <'\\''/dev/input/event4'\\''>}'"
        );
    }

    #[test]
    fn test_build_bridge_key_command() {
        let command = build_bridge_key_command("home");

        assert_eq!(
            command,
            "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/dbus/user_bus_socket gdbus call --session --dest 'ru.kotdath.AudbBridge' --object-path '/ru/kotdath/AudbBridge' --method 'ru.kotdath.AudbBridge.Key' 'home'"
        );
    }
}
