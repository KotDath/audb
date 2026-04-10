use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub auth: String,
    /// Cached current devel-su password used for root-capable commands
    #[serde(default = "default_root_password")]
    pub root_password: String,
    pub arch: DeviceArch,
    pub kind: DeviceKind,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emulator: Option<QemuEmulatorConfig>,
}

fn default_port() -> u16 {
    22
}

fn default_enabled() -> bool {
    true
}

fn default_root_password() -> String {
    String::new()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceArch {
    AuroraArm,
    AuroraArm64,
    AuroraX86_64,
}

impl std::fmt::Display for DeviceArch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceArch::AuroraArm => write!(f, "aurora-arm"),
            DeviceArch::AuroraArm64 => write!(f, "aurora-arm64"),
            DeviceArch::AuroraX86_64 => write!(f, "aurora-x86_64"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceKind {
    Physical,
    QemuEmulator,
}

impl std::fmt::Display for DeviceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceKind::Physical => write!(f, "physical"),
            DeviceKind::QemuEmulator => write!(f, "qemu-emulator"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QemuEmulatorConfig {
    pub sdk_root: String,
    pub release: String,
    pub base_image: String,
    pub overlay_image: String,
    pub ssh_host: String,
    pub ssh_port: u16,
    pub ssh_key: String,
    pub qmp_socket: String,
    pub pidfile: String,
    pub vm_name: String,
    pub mac: String,
    pub memory_mb: u32,
    pub cpus: u32,
    pub framebuffer_width: u32,
    pub framebuffer_height: u32,
    pub abs_max: u32,
    pub display_profile: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DevicesConfig {
    pub schema_version: u32,
    pub devices: Vec<Device>,
}

pub enum DeviceIdentifier {
    Index(usize),
    Host(String),
    Name(String),
}

impl DeviceIdentifier {
    pub fn parse(s: &str) -> Self {
        if let Ok(idx) = s.parse::<usize>() {
            return DeviceIdentifier::Index(idx);
        }

        if s.parse::<std::net::IpAddr>().is_ok() {
            return DeviceIdentifier::Host(s.to_string());
        }

        DeviceIdentifier::Name(s.to_string())
    }
}

impl Device {
    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.host.clone())
    }

    pub fn is_emulator(&self) -> bool {
        self.kind == DeviceKind::QemuEmulator
    }

    pub fn auth_path(&self) -> PathBuf {
        PathBuf::from(shellexpand::tilde(&self.auth).to_string())
    }

    pub fn emulator_config(&self) -> Option<&QemuEmulatorConfig> {
        self.emulator.as_ref()
    }
}

pub fn generate_device_id(kind: &DeviceKind) -> String {
    let prefix = match kind {
        DeviceKind::Physical => "physical",
        DeviceKind::QemuEmulator => "emulator",
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{:x}", prefix, now)
}

/// Log level for journalctl filtering (Android/iOS style + journalctl native)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    V,
    D,
    I,
    W,
    E,
    F,
    Debug,
    Info,
    Notice,
    Warning,
    Err,
    Crit,
    Alert,
    Emerg,
}

impl LogLevel {
    pub fn to_journalctl_priority(&self) -> &str {
        match self {
            Self::V | Self::D | Self::Debug => "debug",
            Self::I | Self::Info => "info",
            Self::Notice => "notice",
            Self::W | Self::Warning => "warning",
            Self::E | Self::Err => "err",
            Self::F | Self::Crit => "crit",
            Self::Alert => "alert",
            Self::Emerg => "emerg",
        }
    }
}
