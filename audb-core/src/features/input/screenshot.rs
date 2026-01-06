// Screenshot command implementation for Aurora OS devices
//
// This feature requires root access (devel-su) to call the D-Bus screenshot service.

use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::{
    session::DeviceSession,
    types::DeviceIdentifier,
};
use anyhow::{Context, Result};
use chrono::Utc;
use std::path::PathBuf;

pub async fn execute() -> Result<()> {
    // Get current device and establish session
    let current_host = DeviceState::get_current()?;
    let device_id = DeviceIdentifier::Host(current_host);
    let device = DeviceStore::find(&device_id)?;

    let mut session = DeviceSession::connect(&device)
        .context("Failed to connect to device")?;

    // Generate timestamped filename
    let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
    let remote_filename = format!("/home/defaultuser/Pictures/Screenshots/audb_screenshot_{}.png", timestamp);
    let remote_path = PathBuf::from(&remote_filename);

    // Build D-Bus command (without devel-su wrapper - exec_as_root adds it)
    let dbus_command = format!(
        "dbus-send --session --print-reply \
         --dest=org.nemomobile.lipstick \
         /org/nemomobile/lipstick/screenshot \
         org.nemomobile.lipstick.saveScreenshot \
         string:\"{}\"",
        remote_filename
    );

    // Execute D-Bus call using devel-su for root access
    session.exec_as_root(&dbus_command)
        .context("Screenshot requires root access. Set root password using: audb device add")?;

    // Read the screenshot file as base64 (file is owned by root)
    let base64_data = session.read_file_base64(&remote_path)
        .context("Failed to read screenshot file")?;

    // Print base64 data to stdout
    print!("{}", base64_data);

    // Cleanup remote file
    let cleanup_cmd = format!("rm -f {}", remote_filename);
    let _ = session.exec_as_root(&cleanup_cmd);

    Ok(())
}
