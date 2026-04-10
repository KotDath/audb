use crate::tools::types::{Device, DeviceIdentifier, DevicesConfig};
use anyhow::{anyhow, Result};
use directories::BaseDirs;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

pub struct DeviceStore;

const CURRENT_SCHEMA_VERSION: u32 = 2;

impl DeviceStore {
    pub fn config_path() -> Result<PathBuf> {
        let base_dirs =
            BaseDirs::new().ok_or_else(|| anyhow!("Could not determine home directory"))?;
        let config_dir = base_dirs.config_dir().join("audb");
        fs::create_dir_all(&config_dir)?;
        Ok(config_dir.join("devices.json"))
    }

    pub fn load() -> Result<DevicesConfig> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(DevicesConfig {
                schema_version: CURRENT_SCHEMA_VERSION,
                devices: vec![],
            });
        }

        let content = fs::read_to_string(&path)?;
        let value: Value = serde_json::from_str(&content)?;
        let schema_version = value
            .get("schema-version")
            .or_else(|| value.get("schema_version"))
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                anyhow!(
                "Legacy devices config detected at {}. Recreate devices with the new schema (v2).",
                path.display()
            )
            })?;
        if schema_version != CURRENT_SCHEMA_VERSION as u64 {
            return Err(anyhow!(
                "Unsupported devices config schema version {} at {}. Expected {}.",
                schema_version,
                path.display(),
                CURRENT_SCHEMA_VERSION
            ));
        }
        let config: DevicesConfig = serde_json::from_str(&content)?;
        Ok(config)
    }

    pub fn save(config: &DevicesConfig) -> Result<()> {
        let path = Self::config_path()?;
        let content = serde_json::to_string_pretty(config)?;
        fs::write(&path, content)?;
        Ok(())
    }

    pub fn add(device: Device) -> Result<()> {
        let mut config = Self::load()?;

        if config.devices.iter().any(|d| d.id == device.id) {
            return Err(anyhow!("Device with id {} already exists", device.id));
        }

        if let Some(name) = &device.name {
            if config.devices.iter().any(|d| d.name.as_ref() == Some(name)) {
                return Err(anyhow!("Device with name '{}' already exists", name));
            }
        }

        config.devices.push(device);
        Self::save(&config)?;
        Ok(())
    }

    pub fn remove(identifier: &DeviceIdentifier) -> Result<Device> {
        let mut config = Self::load()?;
        let device = Self::find_device(&config.devices, identifier)?;
        let removed_device = device.clone();

        config.devices.retain(|d| d.id != removed_device.id);
        Self::save(&config)?;
        Ok(removed_device)
    }

    pub fn update(device: Device) -> Result<()> {
        let mut config = Self::load()?;
        let Some(index) = config.devices.iter().position(|d| d.id == device.id) else {
            return Err(anyhow!("Device with id {} not found", device.id));
        };

        if let Some(name) = &device.name {
            if config
                .devices
                .iter()
                .enumerate()
                .any(|(idx, d)| idx != index && d.name.as_ref() == Some(name))
            {
                return Err(anyhow!("Device with name '{}' already exists", name));
            }
        }

        config.devices[index] = device;
        Self::save(&config)?;
        Ok(())
    }

    pub fn find(identifier: &DeviceIdentifier) -> Result<Device> {
        let config = Self::load()?;
        Self::find_device(&config.devices, identifier)
    }

    pub fn list() -> Result<Vec<Device>> {
        let config = Self::load()?;
        Ok(config.devices)
    }

    pub fn list_enabled() -> Result<Vec<Device>> {
        let config = Self::load()?;
        Ok(config.devices.into_iter().filter(|d| d.enabled).collect())
    }

    fn find_device(devices: &[Device], identifier: &DeviceIdentifier) -> Result<Device> {
        match identifier {
            DeviceIdentifier::Index(idx) => devices
                .get(*idx)
                .cloned()
                .ok_or_else(|| anyhow!("Device index {} not found", idx)),
            DeviceIdentifier::Host(host) => devices
                .iter()
                .find(|d| d.host == *host || d.id == *host)
                .cloned()
                .ok_or_else(|| anyhow!("Device with host/id {} not found", host)),
            DeviceIdentifier::Name(name) => devices
                .iter()
                .find(|d| d.name.as_ref() == Some(name) || d.id == *name)
                .cloned()
                .ok_or_else(|| anyhow!("Device with name/id '{}' not found", name)),
        }
    }
}
