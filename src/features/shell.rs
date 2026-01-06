// Shell command implementation for Aurora OS devices
//
// Execute arbitrary commands on remote devices, similar to adb shell.

use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::ssh::SshClient;
use crate::tools::types::DeviceIdentifier;
use anyhow::{anyhow, Result};

pub async fn execute(as_root: bool, command: String) -> Result<()> {
    if command.is_empty() {
        return Err(anyhow!("Command required. Usage: audb shell <command>"));
    }

    // Get device
    let current_host = DeviceState::get_current()?;
    let device_id = DeviceIdentifier::Host(current_host);
    let device = DeviceStore::find(&device_id)?;

    // Connect
    let mut session = SshClient::connect(&device.host, device.port, &device.auth_path())?;

    // Execute command
    let output = if as_root {
        SshClient::exec_as_devel_su(&mut session, &command, &device.root_password)?
    } else {
        SshClient::exec(&mut session, &command)?
    };

    // Print output
    for line in output {
        println!("{}", line);
    }

    Ok(())
}
