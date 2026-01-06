use anyhow::{anyhow, Result};
use directories::BaseDirs;
use std::fs;
use std::path::PathBuf;

pub struct DeviceState;

impl DeviceState {
    pub fn state_path() -> Result<PathBuf> {
        let base_dirs = BaseDirs::new().ok_or_else(|| anyhow!("Could not determine home directory"))?;
        let config_dir = base_dirs.config_dir().join("audb");
        fs::create_dir_all(&config_dir)?;
        Ok(config_dir.join("current_device"))
    }

    pub fn get_current() -> Result<String> {
        let path = Self::state_path()?;
        if !path.exists() {
            return Err(anyhow!("No device selected. Use 'audb select <identifier>' to select a device"));
        }

        let host = fs::read_to_string(&path)?.trim().to_string();
        if host.is_empty() {
            return Err(anyhow!("No device selected"));
        }

        Ok(host)
    }

    pub fn set_current(host: &str) -> Result<()> {
        let path = Self::state_path()?;
        fs::write(&path, host)?;
        Ok(())
    }

    pub fn clear_current() -> Result<()> {
        let path = Self::state_path()?;
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
}
