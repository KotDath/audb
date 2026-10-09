//! Persistent device registry. Mutations are locked and committed by atomic rename.
use crate::{config::EmulatorConfig, CoreError, CoreResult};
use audb_protocol::ErrorCode;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub const REGISTRY_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Emulator,
    Physical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmulatorOptions {
    pub qmp_socket: PathBuf,
    pub sdk_root: PathBuf,
    pub emulator_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceConfig {
    pub id: String,
    pub name: String,
    pub kind: DeviceKind,
    pub host: String,
    pub ssh_port: u16,
    pub ssh_user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_key: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emulator: Option<EmulatorOptions>,
}

impl From<EmulatorConfig> for DeviceConfig {
    fn from(c: EmulatorConfig) -> Self {
        Self {
            id: c.id,
            name: c.name,
            kind: DeviceKind::Emulator,
            host: c.host,
            ssh_port: c.ssh_port,
            ssh_user: c.ssh_user,
            ssh_key: Some(c.ssh_key),
            root_user: Some(c.root_user),
            emulator: Some(EmulatorOptions {
                qmp_socket: c.qmp_socket,
                sdk_root: c.sdk_root,
                emulator_name: c.emulator_name,
            }),
        }
    }
}

impl DeviceConfig {
    pub fn validate(&self, check_key: bool) -> CoreResult<()> {
        if self.id.is_empty()
            || !self
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        {
            return Err(CoreError::invalid(
                "Device ID must contain only letters, digits, '-', '_' or '.'",
            ));
        }
        if self.name.trim().is_empty() || self.ssh_port == 0 {
            return Err(CoreError::invalid(
                "Device name must not be empty and SSH port must be nonzero",
            ));
        }
        for (label, value) in [("host", &self.host), ("SSH user", &self.ssh_user)] {
            if value.is_empty()
                || value.starts_with('-')
                || value.chars().any(|c| c.is_control() || c.is_whitespace())
            {
                return Err(CoreError::invalid(format!("Invalid {label}")));
            }
        }
        if self.host.contains(['@', '/', '\\']) {
            return Err(CoreError::invalid(
                "Host must be an address or SSH profile name",
            ));
        }
        if let Some(e) = &self.emulator {
            if !e.qmp_socket.is_absolute()
                || !e.sdk_root.is_absolute()
                || e.emulator_name.is_empty()
                || e.emulator_name.contains(['/', '\\'])
                || e.emulator_name.chars().any(char::is_control)
            {
                return Err(CoreError::invalid(
                    "Emulator requires absolute SDK/QMP paths and a valid emulator name",
                ));
            }
        }
        if let Some(user) = &self.root_user {
            if user.is_empty()
                || user.starts_with('-')
                || user.chars().any(|c| c.is_control() || c.is_whitespace())
            {
                return Err(CoreError::invalid("Invalid root SSH user"));
            }
        }
        match (self.kind, &self.emulator) {
            (DeviceKind::Physical, Some(_)) => {
                return Err(CoreError::invalid(
                    "Physical devices cannot have emulator options",
                ))
            }
            (DeviceKind::Emulator, None) => {
                return Err(CoreError::invalid("Emulator options are required"))
            }
            _ => {}
        }
        if self.kind == DeviceKind::Emulator && (self.ssh_key.is_none() || self.root_user.is_none())
        {
            return Err(CoreError::invalid(
                "Emulator requires an SSH key and root user",
            ));
        }
        if check_key {
            if let Some(key) = &self.ssh_key {
                if !key.is_file() {
                    return Err(CoreError::new(
                        ErrorCode::NotFound,
                        format!("SSH key not found: {}", key.display()),
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn emulator_config(&self) -> CoreResult<EmulatorConfig> {
        let e = self.emulator.as_ref().ok_or_else(|| {
            CoreError::new(
                ErrorCode::CapabilityUnavailable,
                "This operation requires an emulator",
            )
        })?;
        Ok(EmulatorConfig {
            id: self.id.clone(),
            name: self.name.clone(),
            host: self.host.clone(),
            ssh_port: self.ssh_port,
            ssh_key: self
                .ssh_key
                .clone()
                .ok_or_else(|| CoreError::invalid("Missing SSH key"))?,
            ssh_user: self.ssh_user.clone(),
            root_user: self
                .root_user
                .clone()
                .ok_or_else(|| CoreError::invalid("Missing root user"))?,
            qmp_socket: e.qmp_socket.clone(),
            sdk_root: e.sdk_root.clone(),
            emulator_name: e.emulator_name.clone(),
            config_file: None,
            registry_device: true,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceRegistry {
    pub version: u32,
    pub default_device: Option<String>,
    pub devices: Vec<DeviceConfig>,
}

impl DeviceRegistry {
    pub fn path() -> CoreResult<PathBuf> {
        // Older audb releases used devices.json with an incompatible schema and
        // credentials. Keep that file untouched rather than guessing its meaning.
        Ok(EmulatorConfig::config_path()?.with_file_name("devices-v1.json"))
    }
    pub fn load() -> CoreResult<Self> {
        Self::load_at(&Self::path()?)
    }
    pub fn load_at(path: &Path) -> CoreResult<Self> {
        // Initialization is serialized with mutations, including legacy backup creation.
        Self::transaction_at(path, |_| Ok(()))
    }
    pub fn transaction<F>(f: F) -> CoreResult<Self>
    where
        F: FnOnce(&mut Self) -> CoreResult<()>,
    {
        Self::transaction_at(&Self::path()?, f)
    }
    pub fn transaction_at<F>(path: &Path, f: F) -> CoreResult<Self>
    where
        F: FnOnce(&mut Self) -> CoreResult<()>,
    {
        let parent = path
            .parent()
            .ok_or_else(|| CoreError::invalid("Registry needs a parent directory"))?;
        fs::create_dir_all(parent)?;
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        fs2::FileExt::lock_exclusive(&lock)?;
        let existed = path.exists();
        let mut registry = if existed {
            serde_json::from_slice::<Self>(&fs::read(path)?)?
        } else {
            let legacy = parent.join("emulator.json");
            let mut emulator = if legacy.exists() {
                let raw = fs::read(&legacy)?;
                let backup = parent.join("emulator.json.pre-registry.bak");
                if !backup.exists() {
                    atomic_write(&backup, &raw)?;
                }
                serde_json::from_slice::<EmulatorConfig>(&raw)?
            } else {
                EmulatorConfig::default()
            };
            emulator.id = crate::config::EMULATOR_ID.into();
            Self {
                version: REGISTRY_VERSION,
                default_device: Some(emulator.id.clone()),
                devices: vec![emulator.into()],
            }
        };
        registry.validate()?;
        let before = serde_json::to_vec_pretty(&registry)?;
        f(&mut registry)?;
        registry.validate()?;
        let after = serde_json::to_vec_pretty(&registry)?;
        if !existed || before != after {
            atomic_write(path, &after)?;
        }
        Ok(registry)
    }
    pub fn validate(&self) -> CoreResult<()> {
        if self.version != REGISTRY_VERSION {
            return Err(CoreError::new(
                ErrorCode::ProtocolMismatch,
                format!("Unsupported registry version: {}", self.version),
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for d in &self.devices {
            d.validate(false)?;
            if !ids.insert(&d.id) {
                return Err(CoreError::invalid(format!("Duplicate device ID: {}", d.id)));
            }
        }
        if let Some(id) = &self.default_device {
            self.get(id)?;
        }
        Ok(())
    }
    pub fn get(&self, id: &str) -> CoreResult<&DeviceConfig> {
        self.devices.iter().find(|d| d.id == id).ok_or_else(|| {
            CoreError::new(ErrorCode::DeviceNotFound, format!("Device not found: {id}"))
        })
    }
    pub fn resolve(&self, explicit: Option<&str>) -> CoreResult<&DeviceConfig> {
        if let Some(id) = explicit.or(self.default_device.as_deref()) {
            return self.get(id);
        }
        if self.devices.len() == 1 {
            return Ok(&self.devices[0]);
        }
        Err(CoreError::new(
            ErrorCode::DeviceRequired,
            "Specify --device ID or use select ID",
        ))
    }
    pub fn add(&mut self, device: DeviceConfig) -> CoreResult<()> {
        device.validate(true)?;
        if self.devices.iter().any(|d| d.id == device.id) {
            return Err(CoreError::invalid(format!(
                "Device already exists: {}",
                device.id
            )));
        }
        self.devices.push(device);
        Ok(())
    }
    pub fn remove(&mut self, id: &str) -> CoreResult<()> {
        self.get(id)?;
        self.devices.retain(|d| d.id != id);
        if self.default_device.as_deref() == Some(id) {
            self.default_device = None;
        }
        Ok(())
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> CoreResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::invalid("Missing parent directory"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|e| CoreError::runtime(e.to_string()))?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn physical(id: &str) -> DeviceConfig {
        DeviceConfig {
            id: id.into(),
            name: id.into(),
            kind: DeviceKind::Physical,
            host: "192.168.2.44".into(),
            ssh_port: 22,
            ssh_user: "defaultuser".into(),
            ssh_key: None,
            root_user: None,
            emulator: None,
        }
    }
    #[test]
    fn migration_preserves_options_and_original() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = EmulatorConfig {
            ssh_port: 2345,
            emulator_name: "custom".into(),
            ..Default::default()
        };
        let raw = serde_json::to_vec(&legacy).unwrap();
        fs::write(dir.path().join("emulator.json"), &raw).unwrap();
        let path = dir.path().join("devices.json");
        let registry = DeviceRegistry::load_at(&path).unwrap();
        assert_eq!(registry.resolve(None).unwrap().ssh_port, 2345);
        assert_eq!(
            registry
                .get("emulator")
                .unwrap()
                .emulator
                .as_ref()
                .unwrap()
                .emulator_name,
            "custom"
        );
        assert_eq!(fs::read(dir.path().join("emulator.json")).unwrap(), raw);
        assert_eq!(
            fs::read(dir.path().join("emulator.json.pre-registry.bak")).unwrap(),
            raw
        );
        DeviceRegistry::transaction_at(&path, |r| r.add(physical("phone"))).unwrap();
        assert_eq!(DeviceRegistry::load_at(&path).unwrap().devices.len(), 2);
    }
    #[test]
    fn resolution_and_failed_mutation_are_safe() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let mut r = DeviceRegistry::transaction_at(&path, |r| {
            r.add(physical("one"))?;
            r.add(physical("two"))?;
            r.remove("emulator")
        })
        .unwrap();
        assert_eq!(r.resolve(None).unwrap_err().code, ErrorCode::DeviceRequired);
        assert_eq!(
            r.resolve(Some("missing")).unwrap_err().code,
            ErrorCode::DeviceNotFound
        );
        r.default_device = Some("one".into());
        assert_eq!(r.resolve(Some("two")).unwrap().id, "two");
        assert_eq!(r.default_device.as_deref(), Some("one"));
        let before = fs::read(&path).unwrap();
        assert!(DeviceRegistry::transaction_at(&path, |r| {
            r.devices[0].host.clear();
            Ok(())
        })
        .is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    #[test]
    fn concurrent_mutations_do_not_lose_devices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("devices.json");
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    DeviceRegistry::transaction_at(&path, |r| {
                        r.add(physical(&format!("phone-{i}")))
                    })
                    .unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(DeviceRegistry::load_at(&path).unwrap().devices.len(), 9);
    }
}
