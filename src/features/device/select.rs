use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::types::DeviceIdentifier;
use anyhow::{anyhow, Result};

pub async fn execute(identifier: &str) -> Result<()> {
    let device_id = DeviceIdentifier::parse(identifier);
    let device = DeviceStore::find(&device_id)?;

    if !device.enabled {
        return Err(anyhow!("Device is disabled. Cannot select disabled devices."));
    }

    DeviceState::set_current(&device.host)?;

    println!("\x1b[1m\x1b[32msuccess\x1b[0m: Selected device: {}", device.display_name());
    println!("  Host: {}", device.host);
    println!("  Port: {}", device.port);
    println!("  Platform: {}", device.platform);

    Ok(())
}
