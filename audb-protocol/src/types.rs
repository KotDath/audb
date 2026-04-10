use serde::{Deserialize, Serialize};

/// Request from client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    pub command: Command,
}

/// Response from server to client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub result: CommandResult,
}

/// Commands that can be sent to the server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    /// Test server connection (ping)
    Ping,
    /// Execute shell command on device
    Shell {
        device: String,
        root: bool,
        command: String,
    },
    /// Install RPM package on device
    Install {
        device: String,
        rpm_path: String,
        rpm_data: Vec<u8>,
    },
    /// Tap at coordinates on device
    Tap {
        device: String,
        x: u16,
        y: u16,
        /// Optional: direct evdev device path for fast mode (e.g., "/dev/input/event4" or "auto")
        event_device: Option<String>,
        /// Optional: duration in milliseconds for long press (default: 30ms)
        duration_ms: Option<u32>,
    },
    /// Swipe gesture on device
    Swipe {
        device: String,
        mode: SwipeMode,
        /// Optional: direct evdev device path for fast mode
        event_device: Option<String>,
        /// Optional: explicit number of movement frames
        steps: Option<u32>,
        /// Optional: total gesture duration
        duration_ms: Option<u32>,
        /// Optional: time to hold the finger before movement
        hold_ms: Option<u32>,
    },
    /// Send key event (back, home, power, volume, etc.)
    Key {
        device: String,
        /// Key name (back, home, power, volumeup, volumedown, etc.)
        key_name: String,
    },
    /// Set clipboard text through AudbBridge
    ClipboardSet { device: String, text: String },
    /// Set clipboard text and trigger the visible keyboard paste action
    ClipboardPaste { device: String, text: String },
    /// Get clipboard text through AudbBridge
    ClipboardGet { device: String },
    /// Take screenshot of device
    Screenshot { device: String },
    /// Launch application on device
    Launch { device: String, app_name: String },
    /// Stop application on device
    Stop { device: String, app_name: String },
    /// Retrieve device logs
    Logs { device: String, args: LogsArgs },
    /// Uninstall package from device
    Uninstall {
        device: String,
        package_name: String,
    },
    /// List installed packages on device
    Packages {
        device: String,
        /// Filter packages by name pattern
        filter: Option<String>,
    },
    /// Push file to device
    Push {
        device: String,
        local_path: String,
        remote_path: String,
        /// File data (binary)
        data: Vec<u8>,
    },
    /// Pull file from device
    Pull { device: String, remote_path: String },
    /// Get device information
    Info {
        device: String,
        /// Info category: device, cpu, memory, battery, storage, features, sim (None = all)
        category: Option<String>,
    },
    /// Get server status
    ServerStatus,
    /// Shutdown server
    KillServer,
    /// Force reconnection to device(s)
    Reconnect { device: Option<String> },
    /// Start emulator runtime
    EmulatorStart { device: Option<String> },
    /// Stop emulator runtime
    EmulatorStop { device: Option<String> },
    /// Show emulator runtime status
    EmulatorStatus { device: Option<String> },
    /// Open URL on device (browser, file, etc.)
    Open {
        device: String,
        /// URL to open (https://, file://, tel:, etc.)
        url: String,
    },
}

/// Swipe mode (coordinates or direction)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SwipeMode {
    Coords { x1: u16, y1: u16, x2: u16, y2: u16 },
    Direction(SwipeDirection),
}

/// Swipe direction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SwipeDirection {
    Left,
    Right,
    Up,
    Down,
    LongUp,
    LongDown,
}

/// Log retrieval arguments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogsArgs {
    pub lines: usize,
    pub priority: Option<String>,
    pub unit: Option<String>,
    pub grep: Option<String>,
    pub since: Option<String>,
    pub clear: bool,
    pub force: bool,
    pub kernel: bool,
}

/// Result of command execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandResult {
    Success { output: CommandOutput },
    Error { message: String, kind: ErrorKind },
}

/// Output from command execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandOutput {
    Lines(Vec<String>),
    Binary(Vec<u8>),
    Status(ServerStatus),
    DeviceInfo(DeviceInfo),
    EmulatorStatus(EmulatorStatus),
    Unit,
}

/// Device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_model: String,
    pub os_version: String,
    pub screen_resolution: String,
    pub cpu_model: String,
    pub cpu_cores: u32,
    pub cpu_max_clock: u32,
    pub ram_total_mb: u64,
    pub ram_available_mb: u64,
    pub ram_free_mb: u64,
    pub ram_cached_mb: u64,
    pub ram_buffers_mb: u64,
    pub battery_level: u32,
    pub battery_state: String,
    pub has_nfc: bool,
    pub has_bluetooth: bool,
    pub has_wlan: bool,
    pub has_gnss: bool,
    pub main_camera_mp: f64,
    pub frontal_camera_mp: f64,
    pub internal_storage_total_mb: u64,
    pub internal_storage_free_mb: u64,
}

/// Server status information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub pid: u32,
    pub uptime_secs: u64,
    pub socket_path: String,
    pub devices: Vec<DeviceStatus>,
}

/// Device connection status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub id: String,
    pub name: Option<String>,
    pub host: String,
    pub port: u16,
    pub arch: DeviceArchInfo,
    pub kind: DeviceKindInfo,
    pub state: ConnectionStateInfo,
    pub stats: ConnectionStats,
    pub emulator: Option<EmulatorStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceArchInfo {
    AuroraArm,
    AuroraArm64,
    AuroraX86_64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceKindInfo {
    Physical,
    QemuEmulator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmulatorStatus {
    pub lifecycle: EmulatorLifecycleStateInfo,
    pub ssh_ready: bool,
    pub qmp_ready: bool,
    pub qmp_input_ready: bool,
    pub qmp_screendump_ready: bool,
    pub geometry: Option<DisplayGeometryInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayGeometryInfo {
    pub native_width: u32,
    pub native_height: u32,
    pub visible_width: u32,
    pub visible_height: u32,
    pub orientation: ScreenOrientationInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenOrientationInfo {
    Portrait,
    Landscape,
    InvertedPortrait,
    InvertedLandscape,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmulatorLifecycleStateInfo {
    Stopped,
    Starting,
    Running,
    Errored,
}

/// Connection state information for reporting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConnectionStateInfo {
    Disconnected,
    Connecting {
        attempt: u32,
    },
    Connected {
        duration_secs: u64,
    },
    Errored {
        error: String,
        retry_in_secs: Option<u64>,
    },
    Disabled,
}

/// Connection statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionStats {
    pub connect_attempts: u64,
    pub successful_commands: u64,
    pub failed_commands: u64,
    pub last_error: Option<String>,
}

/// Error kinds for structured error handling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ErrorKind {
    DeviceNotFound,
    DeviceDisconnected,
    CommandFailed,
    ServerError,
    InvalidRequest,
}
