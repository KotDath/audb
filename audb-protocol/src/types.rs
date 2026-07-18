use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: u64,
    pub protocol_version: u32,
    pub command: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub id: u64,
    pub protocol_version: u32,
    pub result: CommandResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommandResult {
    Success {
        output: CommandOutput,
    },
    Error {
        error: AudbError,
        data: Option<Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CommandOutput {
    Json(Value),
    Text(String),
    Binary(Vec<u8>),
    Empty,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudbError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidArgument,
    NotFound,
    EmulatorOff,
    SshError,
    QmpError,
    RuntimeError,
    AppNotRunning,
    AppWaitTimeout,
    DisplayStateTimeout,
    CapabilityUnavailable,
    UnsupportedInEmulatorOnly,
    ProtocolMismatch,
    InternalError,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = serde_json::to_value(self).map_err(|_| std::fmt::Error)?;
        write!(f, "{}", value.as_str().unwrap_or("INTERNAL_ERROR"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Ping,
    Shutdown,
    QmpStatus {
        socket: Option<String>,
    },
    Tap {
        x: i32,
        y: i32,
        duration_ms: u64,
        socket: Option<String>,
    },
    Swipe {
        args: Vec<String>,
        options: SwipeOptions,
        socket: Option<String>,
    },
    Text {
        text: String,
        delay_ms: u64,
        socket: Option<String>,
    },
    Key {
        name: String,
        socket: Option<String>,
    },
    Screenshot {
        socket: Option<String>,
    },
    Shell {
        root: bool,
        command_line: String,
    },
    Push {
        local_path: String,
        remote_path: String,
    },
    Pull {
        remote_path: String,
    },
    Open {
        url: String,
    },
    Info {
        category: Option<String>,
    },
    Logs {
        options: LogsOptions,
    },
    PackageList {
        filter: Option<String>,
    },
    PackageInstall {
        name: String,
        bytes: Vec<u8>,
    },
    PackageUninstall {
        package: String,
    },
    AppLaunch {
        package: String,
    },
    AppStop {
        package: String,
    },
    AppListRunning,
    AppPid {
        package: String,
    },
    AppWait {
        package: String,
        running: bool,
        timeout_ms: u64,
        interval_ms: u64,
    },
    AppClearData {
        package: String,
        confirm: bool,
    },
    DisplayStatus,
    DisplaySet {
        action: String,
        timeout_ms: u64,
    },
    PerfSnapshot {
        package: String,
        sample_interval_ms: u64,
    },
    PerfMonitor {
        package: String,
        duration_ms: u64,
        interval_ms: u64,
    },
    VisualFps {
        duration_ms: u64,
        interval_ms: u64,
        freeze_threshold_ms: u64,
        socket: Option<String>,
    },
    CrashList {
        package: Option<String>,
        since: Option<String>,
        lines: usize,
    },
    CrashWatch {
        package: String,
        timeout_ms: u64,
        interval_ms: u64,
    },
    CrashClear {
        package: Option<String>,
    },
    SandboxPaths {
        package: String,
    },
    SandboxList {
        package: String,
        root: String,
        path: String,
    },
    SandboxPull {
        package: String,
        root: String,
        path: String,
    },
    SandboxSqlite {
        package: String,
        root: String,
        path: String,
        query: String,
    },
    NetworkStatus,
    NetworkInterfaces,
    NetworkTraffic,
    NetworkProxyGet,
    NetworkProxySet {
        host: String,
        port: u16,
    },
    NetworkProxyClear,
    NetworkOffline {
        enabled: bool,
    },
    LocationSet {
        latitude: f64,
        longitude: f64,
        altitude: f64,
    },
    LocationTrackLoad {
        positions: Vec<TrackPosition>,
        looped: Option<bool>,
        speed: Option<i32>,
        default_interval: Option<bool>,
    },
    LocationTrackAction {
        action: String,
        index: Option<i32>,
        looped: Option<bool>,
        speed: Option<i32>,
        default_interval: Option<bool>,
    },
    SensorList,
    SensorEnable {
        sensor: String,
        enabled: bool,
    },
    SensorVector {
        sensor: String,
        x: i32,
        y: i32,
        z: i32,
    },
    SensorScalar {
        sensor: String,
        value: i32,
    },
    ClipboardStatus,
    ClipboardUnavailable,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwipeOptions {
    pub steps: Option<u32>,
    pub duration_ms: Option<u64>,
    pub hold_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogsOptions {
    pub lines: usize,
    pub priority: Option<String>,
    pub unit: Option<String>,
    pub since: Option<String>,
    pub grep: Option<String>,
    pub kernel: bool,
    pub clear: bool,
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackPosition {
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default)]
    pub altitude: f64,
    #[serde(default = "default_track_interval")]
    pub interval: i32,
}

fn default_track_interval() -> i32 {
    1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable() {
        assert_eq!(
            ErrorCode::CapabilityUnavailable.to_string(),
            "CAPABILITY_UNAVAILABLE"
        );
    }

    #[test]
    fn command_roundtrip_is_typed() {
        let request = Request {
            id: 7,
            protocol_version: PROTOCOL_VERSION,
            command: Command::Tap {
                x: 10,
                y: 20,
                duration_ms: 150,
                socket: None,
            },
        };
        let json = serde_json::to_string(&request).unwrap();
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded.command, Command::Tap { x: 10, y: 20, .. }));
    }
}
