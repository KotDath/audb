// Launch command implementation for Aurora OS applications
//
// Uses RuntimeManager D-Bus API to start applications via:
// gdbus call --system --dest ru.omp.RuntimeManager
//   --object-path /ru/omp/RuntimeManager/Control1
//   --method ru.omp.RuntimeManager.Control1.Start "app_name"

use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::{
    macros::print_info,
    session::DeviceSession,
    types::DeviceIdentifier,
};
use anyhow::{anyhow, Context, Result};

pub async fn execute(app_name: &str) -> Result<()> {
    // Validate app_name
    validate_app_name(app_name)?;

    // Get device and establish session
    let current_host = DeviceState::get_current()?;
    let device_id = DeviceIdentifier::Host(current_host);
    let device = DeviceStore::find(&device_id)?;

    print_info(format!("Launching {} on device {}", app_name, device.display_name()));
    print_info(format!("Connecting to {}:{}...", device.host, device.port));

    let mut session = DeviceSession::connect(&device)
        .context("Failed to connect to device")?;

    // Build D-Bus command to launch app using RuntimeManager
    let launch_command = format!(
        "gdbus call --system --dest ru.omp.RuntimeManager \
         --object-path /ru/omp/RuntimeManager/Control1 \
         --method ru.omp.RuntimeManager.Control1.Start \"{}\"",
        app_name
    );

    print_info("Launching application...");

    let output = session.exec(&launch_command)
        .context("Failed to launch application")?;

    // Display output (shows instance ID and PID)
    for line in &output {
        if !line.is_empty() {
            println!("{}", line);
        }
    }

    print_info("Application launched successfully");
    Ok(())
}

fn validate_app_name(app_name: &str) -> Result<()> {
    if app_name.is_empty() {
        return Err(anyhow!("App name cannot be empty"));
    }
    if !app_name.contains('.') {
        return Err(anyhow!(
            "Invalid app name: '{}'. Expected D-Bus format: ru.domain.AppName",
            app_name
        ));
    }
    if app_name.len() > 255 {
        return Err(anyhow!("App name exceeds D-Bus limit of 255 characters"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_app_name_valid() {
        assert!(validate_app_name("ru.auroraos.MLPackLearning").is_ok());
        assert!(validate_app_name("com.example.App").is_ok());
    }

    #[test]
    fn test_validate_app_name_empty() {
        assert!(validate_app_name("").is_err());
    }

    #[test]
    fn test_validate_app_name_no_dot() {
        assert!(validate_app_name("InvalidAppName").is_err());
    }

    #[test]
    fn test_validate_app_name_too_long() {
        let long_name = "a".repeat(256);
        assert!(validate_app_name(&long_name).is_err());
    }
}
