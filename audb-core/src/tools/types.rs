use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub auth: String,
    /// Root password for devel-su (stored for potential future use)
    /// NOTE: Root automation is not yet implemented - see ssh.rs::exec_as_devel_su
    #[serde(default = "default_root_password")]
    pub root_password: String,
    pub platform: Platform,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
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
pub enum Platform {
    AuroraArm,
    AuroraArm64,
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Platform::AuroraArm => write!(f, "aurora-arm"),
            Platform::AuroraArm64 => write!(f, "aurora-arm64"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DevicesConfig {
    pub aurora_devices: Vec<Device>,
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

    pub fn auth_path(&self) -> PathBuf {
        PathBuf::from(shellexpand::tilde(&self.auth).to_string())
    }
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
