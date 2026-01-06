// Screenshot command implementation for Aurora OS devices
//
// This feature requires root access (devel-su) to call the D-Bus screenshot service.

use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::ssh::SshClient;
use crate::tools::types::DeviceIdentifier;
use anyhow::{anyhow, Result};
use chrono::Utc;
use std::path::PathBuf;

pub async fn execute() -> Result<()> {
    // Get current device
    let current_host = DeviceState::get_current()?;
    let device_id = DeviceIdentifier::Host(current_host);
    let device = DeviceStore::find(&device_id)?;

    // Connect via SSH
    let mut session = SshClient::connect(&device.host, device.port, &device.auth_path())?;

    // Generate timestamped filename
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let remote_filename = format!("/home/defaultuser/Pictures/Screenshots/audb_screenshot_{}.png", timestamp);
    let remote_path = PathBuf::from(&remote_filename);

    // Build D-Bus command (without devel-su wrapper - exec_as_devel_su adds it)
    let dbus_command = format!(
        "dbus-send --session --print-reply \
         --dest=org.nemomobile.lipstick \
         /org/nemomobile/lipstick/screenshot \
         org.nemomobile.lipstick.saveScreenshot \
         string:\"{}\"",
        remote_filename
    );

    // Execute D-Bus call using devel-su for root access
    match SshClient::exec_as_devel_su(&mut session, &dbus_command, &device.root_password) {
        Ok(_) => {
            // Read the screenshot file as base64 (file is owned by root)
            let base64_data = SshClient::read_file_base64(&mut session, &remote_path, &device.root_password)?;

            // Print base64 data to stdout
            print!("{}", base64_data);

            // Cleanup remote file
            let cleanup_cmd = format!("rm -f {}", remote_filename);
            let _ = SshClient::exec_as_devel_su(&mut session, &cleanup_cmd, &device.root_password);

            Ok(())
        }
        Err(e) => {
            // Check if error is related to missing root password
            if e.to_string().contains("Root password not configured") {
                Err(anyhow!(
                    "Screenshot requires root access. {}. \
                    Set root password using: audb device add",
                    e
                ))
            } else {
                Err(e)
            }
        }
    }
}
