use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::types::DeviceIdentifier;
use anyhow::Result;
use dialoguer::Confirm;

pub async fn execute(identifier: &str) -> Result<()> {
    let device_id = DeviceIdentifier::parse(identifier);
    let device = DeviceStore::find(&device_id)?;

    println!("\x1b[1mDevice to remove:\x1b[0m");
    println!("  Name: {}", device.display_name());
    println!("  Host: {}", device.host);
    println!("  Port: {}", device.port);
    println!("  Platform: {}", device.platform);

    let confirmed = Confirm::new()
        .with_prompt("Are you sure you want to remove this device?")
        .default(false)
        .interact()?;

    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }

    // Check if this is the currently selected device
    let current_host = DeviceState::get_current().ok();
    let is_current = current_host.as_ref() == Some(&device.host);

    // Remove device
    DeviceStore::remove(&device_id)?;

    // Clear current device if needed
    if is_current {
        DeviceState::clear_current()?;
        println!("\x1b[1m\x1b[94minfo\x1b[0m: Current device selection cleared");
    }

    println!("\x1b[1m\x1b[32msuccess\x1b[0m: Device removed successfully");
    Ok(())
}
