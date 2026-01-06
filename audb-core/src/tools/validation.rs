use anyhow::{anyhow, Result};
use std::path::Path;

pub fn validate_ip_address(ip: &str) -> Result<()> {
    ip.parse::<std::net::IpAddr>()
        .map(|_| ())
        .map_err(|_| anyhow!("Invalid IP address format"))
}

pub fn validate_port(port: u16) -> Result<()> {
    if port == 0 {
        return Err(anyhow!("Port cannot be 0"));
    }
    Ok(())
}

pub fn validate_ssh_key_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("SSH key file does not exist: {}", path.display()));
    }
    if !path.is_file() {
        return Err(anyhow!("SSH key path is not a file: {}", path.display()));
    }
    Ok(())
}

pub fn validate_rpm_exists(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("RPM file does not exist: {}", path.display()));
    }
    if !path.is_file() {
        return Err(anyhow!("RPM path is not a file: {}", path.display()));
    }
    if path.extension().and_then(|s| s.to_str()) != Some("rpm") {
        return Err(anyhow!("File is not an RPM package: {}", path.display()));
    }
    Ok(())
}
