use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::features::input::scripts::ScriptManager;
use crate::print_info;
use crate::tools::ssh::SshClient;
use crate::tools::types::DeviceIdentifier;
use anyhow::{anyhow, Result};

pub async fn execute(x: u16, y: u16) -> Result<()> {
    // Validate coordinates
    if x > 4096 || y > 4096 {
        return Err(anyhow!("Coordinates out of range: ({}, {}). Max: 4096x4096", x, y));
    }

    // Get device
    let current_host = DeviceState::get_current()?;
    let device_id = DeviceIdentifier::Host(current_host);
    let device = DeviceStore::find(&device_id)?;

    print_info!("Tapping at ({}, {}) on device {}", x, y, device.display_name());

    // Connect
    print_info!("Connecting to {}:{}...", device.host, device.port);
    let mut session = SshClient::connect(&device.host, device.port, &device.auth_path())?;

    // Ensure script present
    ScriptManager::ensure_tap_script(&mut session)?;

    // Execute tap
    let script_path = ScriptManager::tap_script_path();
    let tap_command = format!("python3 {} {} {}", script_path, x, y);

    print_info!("Executing tap...");
    let output = SshClient::exec_as_devel_su(&mut session, &tap_command, &device.root_password)?;

    // Display output
    for line in &output {
        if !line.is_empty() {
            println!("{}", line);
        }
    }

    print_info!("Tap completed successfully");
    Ok(())
}
