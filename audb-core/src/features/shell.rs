// Shell command implementation for Aurora OS devices
//
// Execute arbitrary commands on remote devices, similar to adb shell.

use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::{
    session::DeviceSession,
    types::DeviceIdentifier,
};
use anyhow::{anyhow, Context, Result};

pub async fn execute(as_root: bool, command: String) -> Result<()> {
    if command.is_empty() {
        return Err(anyhow!("Command required. Usage: audb shell <command>"));
    }

    // Get device and establish session
    let current_host = DeviceState::get_current()?;
    let device_id = DeviceIdentifier::Host(current_host);
    let device = DeviceStore::find(&device_id)?;

    let mut session = DeviceSession::connect(&device)
        .context("Failed to connect to device")?;

    // Execute command
    let output = if as_root {
        session.exec_as_root(&command)
            .context("Failed to execute command as root. Set root password using: audb device add")?
    } else {
        session.exec(&command)
            .context("Failed to execute command")?
    };

    // Print output
    for line in output {
        println!("{}", line);
    }

    Ok(())
}
