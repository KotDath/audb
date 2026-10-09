use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 10;

/// One-shot bootstrap credential. Never include it in responses or Debug logs.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RootPassword(String);
impl RootPassword {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for RootPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
impl Drop for RootPassword {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.0.zeroize();
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemPackageOptions {
    pub upgrade: bool,
    #[serde(default)]
    pub reinstall: bool,
    pub check_only: bool,
    pub root_password: Option<RootPassword>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub id: u64,
    pub protocol_version: u32,
    pub device_id: Option<String>,
    pub timeout_ms: u64,
    pub command: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub id: u64,
    pub protocol_version: u32,
    pub device_id: Option<String>,
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
    #[serde(skip)]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidArgument,
    InputNotFocused,
    InputBusy,
    AppNotFound,
    PermissionNotDeclared,
    PromptEnabled,
    PermissionDenied,
    PermissionVerifyFailed,
    PermissionServiceUnavailable,
    DeviceRequired,
    DeviceNotFound,
    RemoteCommandFailed,
    OutcomeUnknown,
    RootAccessRequired,
    AuthenticationFailed,
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
#[serde(
    tag = "operation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PermissionAction {
    List,
    Grant {
        permissions: Vec<String>,
        all_requested: bool,
        disable_prompt: bool,
    },
    Revoke {
        permissions: Vec<String>,
    },
    Reset,
    Prompt {
        enabled: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Permission {
        application_id: String,
        action: PermissionAction,
    },
    Ping,
    Shutdown,
    DeviceStatus,
    Doctor,
    Capabilities,
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
        local_path: String,
        timeout_ms: u64,
    },
    SystemPackageInstall {
        local_path: String,
        options: SystemPackageOptions,
    },
    SetupDevice {
        local_path: String,
        options: SystemPackageOptions,
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
        x: f64,
        y: f64,
        z: f64,
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
    fn root_credentials_are_redacted_in_debug_but_roundtrip_privately() {
        let options = SystemPackageOptions {
            root_password: Some(RootPassword::new("test-credential".into())),
            ..Default::default()
        };
        assert!(!format!("{options:?}").contains("test-credential"));
        let decoded: SystemPackageOptions =
            serde_json::from_str(&serde_json::to_string(&options).unwrap()).unwrap();
        assert_eq!(
            decoded.root_password.as_ref().unwrap().expose(),
            "test-credential"
        );
    }

    #[test]
    fn command_roundtrip_is_typed() {
        let request = Request {
            id: 7,
            device_id: Some("phone".into()),
            timeout_ms: 300_000,
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
        assert_eq!(decoded.device_id.as_deref(), Some("phone"));
        assert_eq!(decoded.timeout_ms, 300_000);
        assert!(matches!(decoded.command, Command::Tap { x: 10, y: 20, .. }));
    }
}
