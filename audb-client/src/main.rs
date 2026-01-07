use anyhow::{anyhow, Result};
use audb_protocol::{recv_message, send_message, Command, CommandOutput, CommandResult, Request, Response};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio::net::UnixStream;

#[macro_export]
macro_rules! exit_error {
    ($($arg:tt)*) => {{
        eprintln!("\x1b[1m\x1b[31merror\x1b[0m: {}", format!($($arg)*));
        std::process::exit(1);
    }};
}

#[derive(Parser)]
#[command(name = "audb")]
#[command(about = "Aurora Debug Bridge - Development and debugging CLI tool for Aurora OS", long_about = None)]
#[command(version)]
struct Cli {
    /// Override device selection (use specific device instead of current)
    #[arg(short = 'd', long, global = true)]
    device: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage Aurora OS devices
    Device {
        #[command(subcommand)]
        action: DeviceCommands,
    },

    /// Package management (install, uninstall, sign, validate)
    Package {
        #[command(subcommand)]
        action: PackageCommands,
    },

    /// Select active device
    Select {
        /// Device identifier (name, IP address, or index)
        identifier: String,
    },

    /// Test server connection (ping)
    Ping,

    /// Start the server daemon manually
    StartServer {
        /// Run in foreground (don't daemonize)
        #[arg(long)]
        foreground: bool,
    },

    /// Stop the server daemon
    KillServer,

    /// Show server status
    ServerStatus,

    /// Execute shell command on device
    Shell {
        /// Run as root (devel-su)
        #[arg(short, long)]
        root: bool,
        /// Command to execute (required)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },

    /// Push file to device
    Push {
        /// Local file path
        local: String,
        /// Remote destination path
        remote: String,
    },

    /// Pull file from device
    Pull {
        /// Remote file path
        remote: String,
        /// Local destination path (optional, defaults to current directory)
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Get device information
    Info {
        /// Info category: device, cpu, memory, battery, storage, features, sim (default: all)
        #[arg(value_name = "CATEGORY")]
        category: Option<String>,
    },

    /// Tap at coordinates on device screen
    Tap {
        /// X coordinate
        x: u16,
        /// Y coordinate
        y: u16,
        /// Direct evdev device for fast mode (e.g., /dev/input/event4 or "auto")
        #[arg(long)]
        event: Option<String>,
        /// Duration in milliseconds for long press (default: 30ms, use 500-1000 for long press)
        #[arg(long)]
        duration: Option<u32>,
    },

    /// Swipe on device screen
    Swipe {
        /// Swipe direction (left, right, up, down) or coordinates (x1 y1 x2 y2)
        #[arg(value_name = "DIRECTION|COORDS")]
        args: Vec<String>,
        /// Direct evdev device for fast mode (e.g., /dev/input/event4 or "auto")
        #[arg(long)]
        event: Option<String>,
    },

    /// Send key event (power, home, back, volume, etc.)
    Key {
        /// Key name: power, home, back, volumeup/vol+, volumedown/vol-, menu, close, lock, unlock
        key_name: String,
    },

    /// Take screenshot of device
    Screenshot {
        /// Output file path (defaults to screenshot_TIMESTAMP.png)
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Launch application on device
    Launch {
        /// Application name (D-Bus format: ru.domain.AppName)
        app_name: String,
    },

    /// Stop application on device
    Stop {
        /// Application name (D-Bus format: ru.domain.AppName)
        app_name: String,
    },

    /// Retrieve device logs
    Logs {
        /// Number of lines to retrieve
        #[arg(short = 'n', long, default_value = "100")]
        lines: usize,

        /// Filter by priority level (0-7 or debug, info, notice, warning, err, crit, alert, emerg)
        #[arg(short, long)]
        priority: Option<String>,

        /// Filter by systemd unit
        #[arg(short, long)]
        unit: Option<String>,

        /// Filter with grep pattern
        #[arg(short, long)]
        grep: Option<String>,

        /// Show logs since timestamp (e.g., "1 hour ago", "2023-01-01")
        #[arg(short, long)]
        since: Option<String>,

        /// Clear all logs (requires --force)
        #[arg(long)]
        clear: bool,

        /// Force clear logs without confirmation
        #[arg(long)]
        force: bool,

        /// Show kernel messages only
        #[arg(short, long)]
        kernel: bool,
    },

    /// Force reconnection to device(s)
    Reconnect {
        /// Device to reconnect (reconnects all if not specified)
        device: Option<String>,
    },

    /// Open URL on device (browser, file, etc.)
    Open {
        /// URL to open (https://, file://, tel:, mailto:, etc.)
        url: String,
    },
}

#[derive(Subcommand)]
enum DeviceCommands {
    /// List all devices
    List {
        /// Show only active (reachable) devices
        #[arg(short, long)]
        active: bool,
    },
    /// Add a new device interactively
    Add,
    /// Remove a device
    Remove {
        /// Device identifier (name, IP address, or index)
        identifier: String,
    },
}

#[derive(Subcommand)]
enum PackageCommands {
    /// Install RPM package on device
    Install {
        /// Path to RPM file
        rpm_path: String,
    },
    /// Uninstall package from device
    Uninstall {
        /// Package name (e.g., ru.domain.AppName)
        package_name: String,
    },
    /// List installed packages on device
    List {
        /// Filter packages by name pattern
        #[arg(short, long)]
        filter: Option<String>,
    },
    /// Sign RPM package with Aurora OS keys (local, uses Docker)
    Sign {
        /// Path to RPM file
        rpm_path: String,
    },
    /// Validate RPM package for Aurora OS compliance (local, uses Docker)
    Validate {
        /// Path to RPM file
        rpm_path: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let device_override = cli.device;

    let result = match cli.command {
        // Device management commands (run locally, not through server)
        Commands::Device { action } => match action {
            DeviceCommands::List { active } => {
                audb_core::features::device::list::execute(active).await
            }
            DeviceCommands::Add => {
                audb_core::features::device::add::execute().await
            }
            DeviceCommands::Remove { identifier } => {
                audb_core::features::device::remove::execute(&identifier).await
            }
        },

        // Package management commands
        Commands::Package { action } => match action {
            PackageCommands::Install { rpm_path } => {
                execute_install_command(device_override, rpm_path).await
            }
            PackageCommands::Uninstall { package_name } => {
                execute_uninstall_command(device_override, package_name).await
            }
            PackageCommands::List { filter } => {
                execute_packages_command(device_override, filter).await
            }
            PackageCommands::Sign { rpm_path } => {
                execute_sign_command(rpm_path).await
            }
            PackageCommands::Validate { rpm_path } => {
                execute_validate_command(rpm_path).await
            }
        },

        Commands::Select { identifier } => {
            audb_core::features::device::select::execute(&identifier).await
        }

        // Server management commands
        Commands::Ping => {
            execute_command(Command::Ping).await
        }
        Commands::StartServer { foreground } => {
            start_server(foreground).await
        }
        Commands::KillServer => {
            kill_server().await
        }
        Commands::ServerStatus => {
            execute_command(Command::ServerStatus).await
        }

        // Device commands (through server)
        Commands::Shell { root, command } => {
            execute_shell_command(device_override, root, command).await
        }
        Commands::Push { local, remote } => {
            execute_push_command(device_override, local, remote).await
        }
        Commands::Pull { remote, output } => {
            execute_pull_command(device_override, remote, output).await
        }
        Commands::Info { category } => {
            execute_info_command(device_override, category).await
        }
        Commands::Tap { x, y, event, duration } => {
            execute_tap_command(device_override, x, y, event, duration).await
        }
        Commands::Swipe { args, event } => {
            execute_swipe_command(device_override, args, event).await
        }
        Commands::Key { key_name } => {
            execute_key_command(device_override, key_name).await
        }
        Commands::Screenshot { output } => {
            execute_screenshot_command(device_override, output).await
        }
        Commands::Launch { app_name } => {
            execute_launch_command(device_override, app_name).await
        }
        Commands::Stop { app_name } => {
            execute_stop_command(device_override, app_name).await
        }
        Commands::Logs {
            lines,
            priority,
            unit,
            grep,
            since,
            clear,
            force,
            kernel,
        } => {
            execute_logs_command(device_override, lines, priority, unit, grep, since, clear, force, kernel).await
        }
        Commands::Reconnect { device } => {
            execute_command(Command::Reconnect { device }).await
        }
        Commands::Open { url } => {
            execute_open_command(device_override, url).await
        }
    };

    if let Err(e) = result {
        exit_error!("{}", e);
    }
}

/// Execute shell command through server
async fn execute_shell_command(device_override: Option<String>, as_root: bool, command_parts: Vec<String>) -> Result<()> {
    let device = get_device(device_override)?;
    let command = command_parts.join(" ");

    execute_command(Command::Shell {
        device,
        root: as_root,
        command,
    }).await
}

/// Execute a command by sending it to the server
async fn execute_command(command: Command) -> Result<()> {
    // Ensure server is running (auto-start if needed)
    ensure_server_running().await?;

    // Connect to server
    let mut stream = connect_to_server().await?;

    // Generate request ID
    let request = Request {
        id: generate_request_id(),
        command,
    };

    // Send request
    send_message(&mut stream, &request).await?;

    // Receive response
    let response: Response = recv_message(&mut stream).await?;

    // Handle response
    handle_response(response)?;

    Ok(())
}

/// Handle server response
fn handle_response(response: Response) -> Result<()> {
    match response.result {
        CommandResult::Success { output } => {
            match output {
                CommandOutput::Lines(lines) => {
                    for line in lines {
                        println!("{}", line);
                    }
                }
                CommandOutput::Binary(data) => {
                    println!("Binary data: {} bytes", data.len());
                }
                CommandOutput::Status(status) => {
                    println!("Server Status:");
                    println!("  PID: {}", status.pid);
                    println!("  Uptime: {} seconds", status.uptime_secs);
                    println!("  Socket: {}", status.socket_path);
                    println!("\nDevices ({}):", status.devices.len());
                    for device in status.devices {
                        let state_str = match &device.state {
                            audb_protocol::ConnectionStateInfo::Disconnected => "disconnected".to_string(),
                            audb_protocol::ConnectionStateInfo::Connecting { attempt } => format!("connecting (attempt {})", attempt),
                            audb_protocol::ConnectionStateInfo::Connected { duration_secs } => format!("connected ({}s)", duration_secs),
                            audb_protocol::ConnectionStateInfo::Errored { error, .. } => format!("error: {}", error),
                            audb_protocol::ConnectionStateInfo::Disabled => "disabled".to_string(),
                        };
                        println!("  {} ({}:{}) - {}",
                            device.name.unwrap_or_else(|| "unnamed".to_string()),
                            device.host,
                            device.port,
                            state_str
                        );
                        if device.stats.failed_commands > 0 || device.stats.last_error.is_some() {
                            println!("    Commands: {} ok, {} failed",
                                device.stats.successful_commands,
                                device.stats.failed_commands
                            );
                            if let Some(ref err) = device.stats.last_error {
                                println!("    Last error: {}", err);
                            }
                        }
                    }
                }
                CommandOutput::DeviceInfo(info) => {
                    // This is handled specially in execute_info_command
                    print_device_info(&info, None);
                }
                CommandOutput::Unit => {
                    // No output
                }
            }
            Ok(())
        }
        CommandResult::Error { message, kind } => {
            // Improve error message for disconnected device
            if message.contains("deadline has elapsed") || message.contains("Channel send error") {
                Err(anyhow!("Device disconnected or unreachable. Check 'audb device list' for status."))
            } else {
                Err(anyhow!("{:?}: {}", kind, message))
            }
        }
    }
}

/// Get the path to the Unix socket
fn socket_path() -> PathBuf {
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/audb-server-{}.sock", uid))
}

/// Connect to the server via Unix socket
async fn connect_to_server() -> Result<UnixStream> {
    let socket_path = socket_path();
    UnixStream::connect(&socket_path)
        .await
        .map_err(|e| anyhow!("Failed to connect to server at {}: {}", socket_path.display(), e))
}

/// Check if the server is running
async fn is_server_running() -> bool {
    connect_to_server().await.is_ok()
}

/// Ensure the server is running, auto-starting if needed
async fn ensure_server_running() -> Result<()> {
    if !is_server_running().await {
        println!("Server not running, starting...");
        start_server(false).await?;

        // Wait for server to be ready (up to 5 seconds)
        for _ in 0..50 {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            if is_server_running().await {
                println!("Server started successfully");
                return Ok(());
            }
        }

        return Err(anyhow!("Server failed to start within timeout"));
    }
    Ok(())
}

/// Start the server daemon
async fn start_server(foreground: bool) -> Result<()> {
    use std::process::Command as ProcessCommand;

    // Find the server binary - check multiple locations
    let server_binary = find_server_binary()?;

    let mut cmd = ProcessCommand::new(&server_binary);

    if foreground {
        cmd.arg("--foreground");
        // Run in foreground, blocking
        let status = cmd.status()?;
        if !status.success() {
            return Err(anyhow!("Server exited with error"));
        }
    } else {
        // Spawn in background
        cmd.spawn()?;
    }

    Ok(())
}

/// Find the audb-server binary
fn find_server_binary() -> Result<PathBuf> {
    // 1. Check if audb-server is in PATH
    if let Ok(output) = std::process::Command::new("which")
        .arg("audb-server")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
    }

    // 2. Check next to the current executable
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            let server_path = dir.join("audb-server");
            if server_path.exists() {
                return Ok(server_path);
            }
        }
    }

    // 3. Check in cargo target directories (development)
    let cargo_paths = [
        "target/debug/audb-server",
        "target/release/audb-server",
        "../target/debug/audb-server",
        "../target/release/audb-server",
    ];

    for path in cargo_paths {
        let p = PathBuf::from(path);
        if p.exists() {
            return Ok(p);
        }
    }

    Err(anyhow!(
        "Could not find audb-server binary. Make sure it's installed or in your PATH."
    ))
}

/// Generate a unique request ID
fn generate_request_id() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64
}

/// Get device to use: override if provided, otherwise current device
fn get_device(device_override: Option<String>) -> Result<String> {
    if let Some(device) = device_override {
        Ok(device)
    } else {
        std::fs::read_to_string(std::path::PathBuf::from(
            shellexpand::tilde("~/.config/audb/current_device").to_string(),
        ))
        .map(|s| s.trim().to_string())
        .map_err(|_| anyhow!("No device selected. Use 'audb device list' and 'audb select <device>' first, or use --device flag"))
    }
}

/// Execute Install command
async fn execute_install_command(device_override: Option<String>, rpm_path: String) -> Result<()> {
    let device = get_device(device_override)?;

    // Read RPM file
    let rpm_data = std::fs::read(&rpm_path)
        .map_err(|e| anyhow!("Failed to read RPM file {}: {}", rpm_path, e))?;

    execute_command(Command::Install {
        device,
        rpm_path,
        rpm_data,
    }).await
}

/// Execute Uninstall command
async fn execute_uninstall_command(device_override: Option<String>, package_name: String) -> Result<()> {
    let device = get_device(device_override)?;

    execute_command(Command::Uninstall {
        device,
        package_name,
    }).await
}

/// Execute Packages command
async fn execute_packages_command(device_override: Option<String>, filter: Option<String>) -> Result<()> {
    let device = get_device(device_override)?;

    execute_command(Command::Packages {
        device,
        filter,
    }).await
}

/// Execute Push command
async fn execute_push_command(device_override: Option<String>, local: String, remote: String) -> Result<()> {
    let device = get_device(device_override)?;

    // Read local file
    let data = std::fs::read(&local)
        .map_err(|e| anyhow!("Failed to read local file {}: {}", local, e))?;

    execute_command(Command::Push {
        device,
        local_path: local,
        remote_path: remote,
        data,
    }).await
}

/// Execute Pull command
async fn execute_pull_command(device_override: Option<String>, remote: String, output: Option<String>) -> Result<()> {
    let device = get_device(device_override)?;

    // Ensure server is running
    ensure_server_running().await?;

    // Connect to server
    let mut stream = connect_to_server().await?;

    // Send pull command
    let request = Request {
        id: generate_request_id(),
        command: Command::Pull {
            device,
            remote_path: remote.clone(),
        },
    };

    send_message(&mut stream, &request).await?;

    // Receive response
    let response: Response = recv_message(&mut stream).await?;

    // Handle pull response specially (binary data)
    match response.result {
        CommandResult::Success { output: CommandOutput::Binary(data) } => {
            // Determine output filename
            let filename = output.unwrap_or_else(|| {
                std::path::Path::new(&remote)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| "pulled_file".to_string())
            });

            // Write to file
            std::fs::write(&filename, &data)?;
            println!("{}: {} bytes pulled to {}", remote, data.len(), filename);
            Ok(())
        }
        CommandResult::Success { output: _ } => {
            Err(anyhow!("Unexpected output format for pull"))
        }
        CommandResult::Error { message, kind } => {
            Err(anyhow!("{:?}: {}", kind, message))
        }
    }
}

/// Execute Info command
async fn execute_info_command(device_override: Option<String>, category: Option<String>) -> Result<()> {
    let device = get_device(device_override)?;

    // Ensure server is running
    ensure_server_running().await?;

    // Connect to server
    let mut stream = connect_to_server().await?;

    // Send info command
    let request = Request {
        id: generate_request_id(),
        command: Command::Info {
            device,
            category: category.clone(),
        },
    };

    send_message(&mut stream, &request).await?;

    // Receive response
    let response: Response = recv_message(&mut stream).await?;

    // Handle response
    match response.result {
        CommandResult::Success { output: CommandOutput::DeviceInfo(info) } => {
            print_device_info(&info, category.as_deref());
            Ok(())
        }
        CommandResult::Success { output: _ } => {
            Err(anyhow!("Unexpected output format for info"))
        }
        CommandResult::Error { message, kind } => {
            Err(anyhow!("{:?}: {}", kind, message))
        }
    }
}

/// Print device info based on category
fn print_device_info(info: &audb_protocol::DeviceInfo, category: Option<&str>) {
    match category {
        Some("device") => {
            println!("Device:");
            println!("  Model: {}", info.device_model);
            println!("  OS Version: {}", info.os_version);
            println!("  Screen: {}", info.screen_resolution);
        }
        Some("cpu") => {
            println!("CPU:");
            println!("  Model: {}", info.cpu_model);
            println!("  Cores: {}", info.cpu_cores);
            println!("  Max Clock: {} MHz", info.cpu_max_clock);
        }
        Some("memory") | Some("mem") | Some("ram") => {
            println!("Memory:");
            println!("  Total: {} MB", info.ram_total_mb);
            println!("  Available: {} MB", info.ram_available_mb);
            println!("  Free: {} MB", info.ram_free_mb);
            println!("  Cached: {} MB", info.ram_cached_mb);
            println!("  Buffers: {} MB", info.ram_buffers_mb);
        }
        Some("battery") | Some("bat") => {
            println!("Battery:");
            println!("  Level: {}%", info.battery_level);
            println!("  State: {}", info.battery_state);
        }
        Some("storage") | Some("disk") => {
            println!("Storage:");
            println!("  Internal Total: {} MB ({:.1} GB)", info.internal_storage_total_mb, info.internal_storage_total_mb as f64 / 1024.0);
            println!("  Internal Free: {} MB ({:.1} GB)", info.internal_storage_free_mb, info.internal_storage_free_mb as f64 / 1024.0);
        }
        Some("features") | Some("hw") => {
            println!("Features:");
            println!("  NFC: {}", if info.has_nfc { "Yes" } else { "No" });
            println!("  Bluetooth: {}", if info.has_bluetooth { "Yes" } else { "No" });
            println!("  WLAN: {}", if info.has_wlan { "Yes" } else { "No" });
            println!("  GNSS: {}", if info.has_gnss { "Yes" } else { "No" });
            println!();
            println!("Cameras:");
            println!("  Main: {:.1} MP", info.main_camera_mp);
            println!("  Frontal: {:.1} MP", info.frontal_camera_mp);
        }
        _ => {
            // Show all info (default)
            println!("Device:");
            println!("  Model: {}", info.device_model);
            println!("  OS Version: {}", info.os_version);
            println!("  Screen: {}", info.screen_resolution);
            println!();
            println!("CPU:");
            println!("  Model: {}", info.cpu_model);
            println!("  Cores: {}", info.cpu_cores);
            println!("  Max Clock: {} MHz", info.cpu_max_clock);
            println!();
            println!("Memory:");
            println!("  Total: {} MB", info.ram_total_mb);
            println!("  Available: {} MB", info.ram_available_mb);
            println!("  Free: {} MB", info.ram_free_mb);
            println!();
            println!("Storage:");
            println!("  Internal: {:.1} GB / {:.1} GB free", 
                info.internal_storage_total_mb as f64 / 1024.0,
                info.internal_storage_free_mb as f64 / 1024.0);
            println!();
            println!("Battery:");
            println!("  Level: {}%", info.battery_level);
            println!("  State: {}", info.battery_state);
            println!();
            println!("Features:");
            println!("  NFC: {}", if info.has_nfc { "Yes" } else { "No" });
            println!("  Bluetooth: {}", if info.has_bluetooth { "Yes" } else { "No" });
            println!("  WLAN: {}", if info.has_wlan { "Yes" } else { "No" });
            println!("  GNSS: {}", if info.has_gnss { "Yes" } else { "No" });
            println!();
            println!("Cameras:");
            println!("  Main: {:.1} MP", info.main_camera_mp);
            println!("  Frontal: {:.1} MP", info.frontal_camera_mp);
        }
    }
}

/// Execute Tap command
async fn execute_tap_command(device_override: Option<String>, x: u16, y: u16, event: Option<String>, duration: Option<u32>) -> Result<()> {
    let device = get_device(device_override)?;

    execute_command(Command::Tap {
        device,
        x,
        y,
        event_device: event,
        duration_ms: duration,
    }).await
}

/// Execute Swipe command
async fn execute_swipe_command(device_override: Option<String>, args: Vec<String>, event: Option<String>) -> Result<()> {
    let device = get_device(device_override)?;

    // Parse swipe arguments
    let mode = if args.len() == 1 {
        // Direction mode
        let direction = match args[0].to_lowercase().as_str() {
            "left" => audb_protocol::SwipeDirection::Left,
            "right" => audb_protocol::SwipeDirection::Right,
            "up" => audb_protocol::SwipeDirection::Up,
            "down" => audb_protocol::SwipeDirection::Down,
            _ => return Err(anyhow!("Invalid swipe direction: {}. Use: left, right, up, or down", args[0])),
        };
        audb_protocol::SwipeMode::Direction(direction)
    } else if args.len() == 4 {
        // Coordinates mode
        let x1 = args[0].parse().map_err(|_| anyhow!("Invalid x1 coordinate: {}", args[0]))?;
        let y1 = args[1].parse().map_err(|_| anyhow!("Invalid y1 coordinate: {}", args[1]))?;
        let x2 = args[2].parse().map_err(|_| anyhow!("Invalid x2 coordinate: {}", args[2]))?;
        let y2 = args[3].parse().map_err(|_| anyhow!("Invalid y2 coordinate: {}", args[3]))?;
        audb_protocol::SwipeMode::Coords { x1, y1, x2, y2 }
    } else {
        return Err(anyhow!("Invalid swipe arguments. Use: <direction> OR <x1> <y1> <x2> <y2>"));
    };

    execute_command(Command::Swipe {
        device,
        mode,
        event_device: event,
    }).await
}

/// Execute Key command
async fn execute_key_command(device_override: Option<String>, key_name: String) -> Result<()> {
    let device = get_device(device_override)?;

    execute_command(Command::Key {
        device,
        key_name,
    }).await
}

/// Execute Screenshot command with special binary handling
async fn execute_screenshot_command(device_override: Option<String>, output: Option<String>) -> Result<()> {
    let device = get_device(device_override)?;

    // Ensure server is running
    ensure_server_running().await?;

    // Connect to server
    let mut stream = connect_to_server().await?;

    // Send screenshot command
    let request = Request {
        id: generate_request_id(),
        command: Command::Screenshot {
            device,
        },
    };

    send_message(&mut stream, &request).await?;

    // Receive response
    let response: Response = recv_message(&mut stream).await?;

    // Handle screenshot response specially
    match response.result {
        CommandResult::Success { output: CommandOutput::Binary(data) } => {
            // Generate output filename
            let filename = output.unwrap_or_else(|| {
                let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
                format!("screenshot_{}.png", timestamp)
            });

            // Write to file
            std::fs::write(&filename, data)?;
            println!("Screenshot saved to: {}", filename);
            Ok(())
        }
        CommandResult::Success { output: _ } => {
            Err(anyhow!("Unexpected output format for screenshot"))
        }
        CommandResult::Error { message, kind } => {
            Err(anyhow!("{:?}: {}", kind, message))
        }
    }
}

/// Execute Launch command
async fn execute_launch_command(device_override: Option<String>, app_name: String) -> Result<()> {
    let device = get_device(device_override)?;

    execute_command(Command::Launch {
        device,
        app_name,
    }).await
}

/// Execute Stop command
async fn execute_stop_command(device_override: Option<String>, app_name: String) -> Result<()> {
    let device = get_device(device_override)?;

    execute_command(Command::Stop {
        device,
        app_name,
    }).await
}

/// Execute Logs command
async fn execute_logs_command(
    device_override: Option<String>,
    lines: usize,
    priority: Option<String>,
    unit: Option<String>,
    grep: Option<String>,
    since: Option<String>,
    clear: bool,
    force: bool,
    kernel: bool,
) -> Result<()> {
    let device = get_device(device_override)?;

    let args = audb_protocol::LogsArgs {
        lines,
        priority,
        unit,
        grep,
        since,
        clear,
        force,
        kernel,
    };

    execute_command(Command::Logs {
        device,
        args,
    }).await
}

/// Kill the server daemon
async fn kill_server() -> Result<()> {
    // Get PID file path
    let pid_file = PathBuf::from(shellexpand::tilde("~/.config/audb/server.pid").to_string());

    if !pid_file.exists() {
        // Check if server is actually running via socket
        if !is_server_running().await {
            println!("Server is not running");
            return Ok(());
        }
        return Err(anyhow!("Server appears to be running but PID file not found"));
    }

    // Read PID from file
    let pid_str = std::fs::read_to_string(&pid_file)?;
    let pid: i32 = pid_str.trim().parse()
        .map_err(|_| anyhow!("Invalid PID in file: {}", pid_str.trim()))?;

    // Send SIGTERM to the process
    unsafe {
        if libc::kill(pid, libc::SIGTERM) == 0 {
            println!("Server (PID {}) terminated", pid);
            // Clean up PID file
            std::fs::remove_file(&pid_file).ok();
            // Clean up socket file
            std::fs::remove_file(socket_path()).ok();
            Ok(())
        } else {
            let errno = *libc::__errno_location();
            if errno == libc::ESRCH {
                // Process doesn't exist, clean up stale files
                println!("Server process not found, cleaning up stale files");
                std::fs::remove_file(&pid_file).ok();
                std::fs::remove_file(socket_path()).ok();
                Ok(())
            } else {
                Err(anyhow!("Failed to kill server (PID {}): errno {}", pid, errno))
            }
        }
    }
}


/// Execute Open command
async fn execute_open_command(device_override: Option<String>, url: String) -> Result<()> {
    let device = get_device(device_override)?;

    execute_command(Command::Open {
        device,
        url,
    }).await
}


/// Execute Sign command (local, uses Docker)
async fn execute_sign_command(rpm_path: String) -> Result<()> {
    use std::process::Command as ProcessCommand;
    use std::path::Path;

    let rpm_path = Path::new(&rpm_path);
    
    // Validate RPM file exists
    if !rpm_path.exists() {
        return Err(anyhow!("RPM file not found: {}", rpm_path.display()));
    }
    
    if !rpm_path.is_file() {
        return Err(anyhow!("Not a file: {}", rpm_path.display()));
    }

    // Get absolute path
    let rpm_path = rpm_path.canonicalize()
        .map_err(|e| anyhow!("Failed to resolve path: {}", e))?;
    
    let rpm_name = rpm_path.file_name()
        .ok_or_else(|| anyhow!("Invalid RPM path"))?
        .to_string_lossy();
    
    let project_dir = rpm_path.parent()
        .ok_or_else(|| anyhow!("Invalid RPM path"))?;

    // Find keys directory
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home".to_string());
    let keys_dir = PathBuf::from(&home).join("AuroraOS").join("package-signing");
    
    if !keys_dir.exists() || !keys_dir.is_dir() {
        return Err(anyhow!(
            "Keys directory not found: {}\n\
            Please ensure you have Aurora OS signing keys installed.\n\
            Expected files:\n\
            - {}/regular_key.pem\n\
            - {}/regular_cert.pem",
            keys_dir.display(),
            keys_dir.display(),
            keys_dir.display()
        ));
    }

    let cert_path = keys_dir.join("regular_cert.pem");
    let key_path = keys_dir.join("regular_key.pem");

    if !cert_path.exists() {
        return Err(anyhow!("Certificate not found: {}", cert_path.display()));
    }
    if !key_path.exists() {
        return Err(anyhow!("Key not found: {}", key_path.display()));
    }

    // Copy keys to project directory temporarily
    let temp_cert = project_dir.join("regular_cert.pem");
    let temp_key = project_dir.join("regular_key.pem");
    
    std::fs::copy(&cert_path, &temp_cert)
        .map_err(|e| anyhow!("Failed to copy certificate: {}", e))?;
    std::fs::copy(&key_path, &temp_key)
        .map_err(|e| anyhow!("Failed to copy key: {}", e))?;

    // Find Aurora SDK Docker image
    let docker_image = find_aurora_docker_image()?;
    
    println!("Signing {} with Aurora SDK...", rpm_name);

    // Generate unique container name
    let container_name = format!("audb-sign-{}", std::process::id());

    // Run Docker container
    let docker_run = ProcessCommand::new("docker")
        .args([
            "run",
            "--rm",
            "--name", &container_name,
            "-v", &format!("{}:/project", project_dir.display()),
            &docker_image,
            "/bin/bash", "-c",
            &format!(
                "rpmsign-external sign --force --key=/project/regular_key.pem --cert=/project/regular_cert.pem /project/{}",
                rpm_name
            ),
        ])
        .output();

    // Clean up temp keys regardless of result
    let _ = std::fs::remove_file(&temp_cert);
    let _ = std::fs::remove_file(&temp_key);

    match docker_run {
        Ok(output) => {
            if output.status.success() {
                println!("Successfully signed: {}", rpm_path.display());
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                Err(anyhow!(
                    "Failed to sign package:\n{}\n{}",
                    stdout.trim(),
                    stderr.trim()
                ))
            }
        }
        Err(e) => Err(anyhow!("Failed to run Docker: {}", e)),
    }
}

/// Find Aurora SDK Docker image
fn find_aurora_docker_image() -> Result<String> {
    use std::process::Command as ProcessCommand;

    // List Docker images and find Aurora SDK
    let output = ProcessCommand::new("docker")
        .args(["images", "--format", "{{.Repository}}:{{.Tag}}"])
        .output()
        .map_err(|e| anyhow!("Failed to list Docker images: {}", e))?;

    if !output.status.success() {
        return Err(anyhow!("Docker command failed. Is Docker installed and running?"));
    }

    let images = String::from_utf8_lossy(&output.stdout);
    
    // Look for Aurora SDK image patterns (prioritize build-tools)
    let mut candidates: Vec<&str> = Vec::new();
    
    for line in images.lines() {
        let lower = line.to_lowercase();
        if lower.contains("aurora") && (lower.contains("build") || lower.contains("sdk") || lower.contains("engine")) {
            candidates.push(line);
        }
    }

    // Prefer build-tools over build-engine
    for candidate in &candidates {
        if candidate.to_lowercase().contains("build-tools") {
            return Ok(candidate.to_string());
        }
    }
    
    // Fall back to any Aurora image
    if let Some(candidate) = candidates.first() {
        return Ok(candidate.to_string());
    }

    Err(anyhow!(
        "Aurora SDK Docker image not found.\n\
        Please ensure you have the Aurora SDK Docker image installed.\n\
        You can pull it from the Aurora OS developer portal."
    ))
}


/// Execute Validate command (local, uses Docker)
async fn execute_validate_command(rpm_path: String) -> Result<()> {
    use std::process::Command as ProcessCommand;
    use std::path::Path;

    let rpm_path = Path::new(&rpm_path);
    
    // Validate RPM file exists
    if !rpm_path.exists() {
        return Err(anyhow!("RPM file not found: {}", rpm_path.display()));
    }
    
    if !rpm_path.is_file() {
        return Err(anyhow!("Not a file: {}", rpm_path.display()));
    }

    // Get absolute path
    let rpm_path = rpm_path.canonicalize()
        .map_err(|e| anyhow!("Failed to resolve path: {}", e))?;
    
    let rpm_name = rpm_path.file_name()
        .ok_or_else(|| anyhow!("Invalid RPM path"))?
        .to_string_lossy();
    
    let project_dir = rpm_path.parent()
        .ok_or_else(|| anyhow!("Invalid RPM path"))?;

    // Find Aurora SDK Docker image
    let docker_image = find_aurora_docker_image()?;
    
    println!("Validating {} with Aurora SDK...", rpm_name);

    // Generate unique container name
    let container_name = format!("audb-validate-{}", std::process::id());

    // Run Docker container
    let docker_run = ProcessCommand::new("docker")
        .args([
            "run",
            "--rm",
            "--name", &container_name,
            "-v", &format!("{}:/project", project_dir.display()),
            &docker_image,
            "/bin/bash", "-c",
            &format!("rpm-validator -p regular /project/{}", rpm_name),
        ])
        .output();

    match docker_run {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            
            // Print output
            if !stdout.is_empty() {
                println!("{}", stdout);
            }
            if !stderr.is_empty() {
                eprintln!("{}", stderr);
            }

            // Check for errors in output
            if stdout.contains("(ERROR)") || stderr.contains("(ERROR)") {
                Err(anyhow!("Validation failed: errors found"))
            } else if output.status.success() {
                println!("Validation passed: no errors found");
                Ok(())
            } else {
                Err(anyhow!("Validation failed with exit code: {:?}", output.status.code()))
            }
        }
        Err(e) => Err(anyhow!("Failed to run Docker: {}", e)),
    }
}
