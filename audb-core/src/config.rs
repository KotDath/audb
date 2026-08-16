use crate::error::{CoreError, CoreResult};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const EMULATOR_ID: &str = "emulator";
pub const EMULATOR_NAME: &str = "Aurora Emulator";
pub const DEFAULT_SDK_ROOT: &str = "/home/kotdath/AuroraOS";
pub const DEFAULT_EMULATOR_NAME: &str = "AuroraOS-5.2.0.180";
pub const DEFAULT_QMP_SOCKET: &str = "/tmp/audb/qmp.sock";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmulatorConfig {
    pub id: String,
    pub name: String,
    pub host: String,
    pub ssh_port: u16,
    pub ssh_key: PathBuf,
    pub ssh_user: String,
    pub root_user: String,
    pub qmp_socket: PathBuf,
    pub sdk_root: PathBuf,
    pub emulator_name: String,
    #[serde(skip)]
    pub(crate) config_file: Option<PathBuf>,
}

impl Default for EmulatorConfig {
    fn default() -> Self {
        let sdk_root = std::env::var_os("AURORA_SDK_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SDK_ROOT));
        Self {
            id: EMULATOR_ID.into(),
            name: EMULATOR_NAME.into(),
            host: "127.0.0.1".into(),
            ssh_port: 2223,
            ssh_key: sdk_root.join("vmshare/ssh/private_keys/sdk"),
            ssh_user: "defaultuser".into(),
            root_user: "root".into(),
            qmp_socket: PathBuf::from(DEFAULT_QMP_SOCKET),
            sdk_root,
            emulator_name: DEFAULT_EMULATOR_NAME.into(),
            config_file: None,
        }
    }
}

impl EmulatorConfig {
    pub fn config_path() -> CoreResult<PathBuf> {
        let base =
            BaseDirs::new().ok_or_else(|| CoreError::runtime("Cannot determine home directory"))?;
        Ok(base.config_dir().join("audb/emulator.json"))
    }

    pub fn load_or_default() -> CoreResult<Self> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(&path)?;
        let mut config: Self = serde_json::from_str(&raw)?;
        config.config_file = Some(path);
        Ok(config)
    }

    pub fn save(&self) -> CoreResult<()> {
        let path = self.storage_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// SDK ships a QEMU binary matching the host CPU architecture
    /// (e.g. `qemu-system-aarch64` on Apple Silicon macOS).
    pub fn qemu_binary_name() -> String {
        format!("qemu-system-{}", std::env::consts::ARCH)
    }

    pub fn qemu_bin(&self) -> PathBuf {
        self.sdk_root
            .join("share/qemu/bin")
            .join(Self::qemu_binary_name())
    }

    pub fn storage_path(&self) -> CoreResult<PathBuf> {
        self.config_file
            .clone()
            .map(Ok)
            .unwrap_or_else(Self::config_path)
    }

    #[cfg(test)]
    pub(crate) fn use_config_file(&mut self, path: PathBuf) {
        self.config_file = Some(path);
    }
    pub fn qemu_real(&self) -> PathBuf {
        self.sdk_root
            .join("share/qemu/bin")
            .join(format!("{}.real", Self::qemu_binary_name()))
    }
    pub fn sfdk(&self) -> PathBuf {
        self.sdk_root.join("bin/sfdk")
    }
    pub fn vmshare(&self) -> PathBuf {
        self.sdk_root
            .join("emulator")
            .join(&self.emulator_name)
            .join("vmshare")
    }

    pub fn with_socket(mut self, socket: Option<&str>) -> Self {
        if let Some(socket) = socket {
            self.qmp_socket = PathBuf::from(socket);
        }
        self
    }

    pub fn validate(&self) -> CoreResult<()> {
        if self.id != EMULATOR_ID {
            return Err(CoreError::invalid("Only the emulator device is supported"));
        }
        if !Path::new(&self.ssh_key).exists() {
            return Err(CoreError::new(
                audb_protocol::ErrorCode::NotFound,
                format!("SSH key not found: {}", self.ssh_key.display()),
            ));
        }
        Ok(())
    }
}

pub fn cache_dir() -> CoreResult<PathBuf> {
    let base =
        BaseDirs::new().ok_or_else(|| CoreError::runtime("Cannot determine home directory"))?;
    Ok(base.cache_dir().join("audb"))
}
