use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::macros::print_info;
use crate::tools::ssh::SshClient;
use crate::tools::types::DeviceIdentifier;
use crate::tools::validation::validate_rpm_exists;
use anyhow::{anyhow, Result};
use std::path::PathBuf;

pub async fn execute(rpm_path: &str) -> Result<()> {
    let local_path = PathBuf::from(rpm_path);

    // Validate RPM file
    validate_rpm_exists(&local_path)?;

    let file_name = local_path
        .file_name()
        .ok_or_else(|| anyhow!("Invalid file name"))?
        .to_string_lossy()
        .to_string();

    // Get currently selected device
    let current_host = DeviceState::get_current()?;
    let device_id = DeviceIdentifier::Host(current_host);
    let device = DeviceStore::find(&device_id)?;

    print_info(format!("Installing {} on device {}", file_name, device.display_name()));

    // Connect to device
    print_info(format!("Connecting to {}:{}...", device.host, device.port));
    let mut session = SshClient::connect(&device.host, device.port, &device.auth_path())?;

    // Upload RPM to Downloads directory (APM requires access to user's Downloads)
    let remote_path = PathBuf::from(format!("/home/defaultuser/Downloads/{}", file_name));
    print_info(format!("Uploading {} to {}...", file_name, remote_path.display()));
    SshClient::upload(&mut session, &local_path, &remote_path)?;

    // Install via gdbus (runs as defaultuser, APM handles permissions via D-Bus)
    print_info("Installing package via APM...");
    let install_command = format!(
        "gdbus call --system --dest ru.omp.APM --object-path /ru/omp/APM --method ru.omp.APM.Install \"{}\" \"{{}}\"",
        remote_path.display()
    );

    let output = SshClient::exec(&mut session, &install_command)?;

    // Display output
    for line in &output {
        if !line.is_empty() {
            println!("{}", line);
        }
    }

    // Cleanup
    print_info("Cleaning up temporary files...");
    let cleanup_command = format!("rm -f {}", remote_path.display());
    SshClient::exec(&mut session, &cleanup_command).ok();

    println!("\n\x1b[1m\x1b[32msuccess\x1b[0m: Package installed successfully");
    Ok(())
}
