// Tap command implementation for Aurora OS devices
//
// This feature requires root access to work properly. The Python script uses
// /dev/uinput which needs root permissions via devel-su.

use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::features::input::scripts::ScriptManager;
use crate::tools::{
    macros::print_info,
    session::DeviceSession,
    types::DeviceIdentifier,
};
use anyhow::{anyhow, Context, Result};

pub async fn execute(x: u16, y: u16) -> Result<()> {
    // Validate coordinates
    if x > 4096 || y > 4096 {
        return Err(anyhow!("Coordinates out of range: ({}, {}). Max: 4096x4096", x, y));
    }

    // Get device and establish session
    let current_host = DeviceState::get_current()?;
    let device_id = DeviceIdentifier::Host(current_host);
    let device = DeviceStore::find(&device_id)?;

    print_info(format!("Tapping at ({}, {}) on device {}", x, y, device.display_name()));
    print_info(format!("Connecting to {}:{}...", device.host, device.port));

    let mut session = DeviceSession::connect(&device)
        .context("Failed to connect to device")?;

    // Ensure tap script is present on device
    ScriptManager::ensure_tap_script_with_session(&mut session)?;

    // Execute tap command using devel-su for root access
    let script_path = ScriptManager::tap_script_path();
    let tap_command = format!("python3 {} {} {}", script_path, x, y);

    print_info("Executing tap with devel-su...");

    let output = session.exec_as_root(&tap_command)
        .context("Tap requires root access. Set root password using: audb device add")?;

    // Display output
    for line in &output {
        if !line.is_empty() {
            println!("{}", line);
        }
    }

    print_info("Tap completed successfully");
    Ok(())
}
