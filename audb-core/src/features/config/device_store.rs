use crate::tools::types::{Device, DeviceIdentifier, DevicesConfig};
use anyhow::{anyhow, Result};
use directories::BaseDirs;
use std::fs;
use std::path::PathBuf;

pub struct DeviceStore;

impl DeviceStore {
    pub fn config_path() -> Result<PathBuf> {
        let base_dirs = BaseDirs::new().ok_or_else(|| anyhow!("Could not determine home directory"))?;
        let config_dir = base_dirs.config_dir().join("audb");
        fs::create_dir_all(&config_dir)?;
        Ok(config_dir.join("devices.json"))
    }

    pub fn load() -> Result<DevicesConfig> {
        let path = Self::config_path()?;
        if !path.exists() {
            return Ok(DevicesConfig {
                aurora_devices: vec![],
            });
        }

        let content = fs::read_to_string(&path)?;
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

        // Check for duplicates by host
        if config.aurora_devices.iter().any(|d| d.host == device.host) {
            return Err(anyhow!("Device with host {} already exists", device.host));
        }

        config.aurora_devices.push(device);
        Self::save(&config)?;
        Ok(())
    }

    pub fn remove(identifier: &DeviceIdentifier) -> Result<Device> {
        let mut config = Self::load()?;
        let device = Self::find_device(&config.aurora_devices, identifier)?;
        let removed_device = device.clone();

        config.aurora_devices.retain(|d| d.host != removed_device.host);
        Self::save(&config)?;
        Ok(removed_device)
    }

    pub fn find(identifier: &DeviceIdentifier) -> Result<Device> {
        let config = Self::load()?;
        Self::find_device(&config.aurora_devices, identifier)
    }

    pub fn list() -> Result<Vec<Device>> {
        let config = Self::load()?;
        Ok(config.aurora_devices)
    }

    pub fn list_enabled() -> Result<Vec<Device>> {
        let config = Self::load()?;
        Ok(config.aurora_devices.into_iter().filter(|d| d.enabled).collect())
    }

    fn find_device(devices: &[Device], identifier: &DeviceIdentifier) -> Result<Device> {
        match identifier {
            DeviceIdentifier::Index(idx) => {
                devices.get(*idx)
                    .cloned()
                    .ok_or_else(|| anyhow!("Device index {} not found", idx))
            }
            DeviceIdentifier::Host(host) => {
                devices.iter()
                    .find(|d| d.host == *host)
                    .cloned()
                    .ok_or_else(|| anyhow!("Device with host {} not found", host))
            }
            DeviceIdentifier::Name(name) => {
                devices.iter()
                    .find(|d| d.name.as_ref() == Some(name))
                    .cloned()
                    .ok_or_else(|| anyhow!("Device with name '{}' not found", name))
            }
        }
    }
}
