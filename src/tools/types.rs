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
