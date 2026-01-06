use crate::features::config::device_store::DeviceStore;
use crate::print_info;
use crate::tools::ssh::SshClient;
use crate::tools::types::{Device, Platform};
use crate::tools::validation::{validate_ip_address, validate_port, validate_ssh_key_exists};
use anyhow::{anyhow, Result};
use dialoguer::{Confirm, Input, Password, Select};
use std::path::PathBuf;

pub async fn execute() -> Result<()> {
    println!("\x1b[1m\x1b[36mAdd Aurora OS Device\x1b[0m\n");

    // Device name (optional)
    let name: String = Input::new()
        .with_prompt("Device name (optional, press Enter to skip)")
        .allow_empty(true)
        .interact_text()?;

    let name = if name.trim().is_empty() {
        None
    } else {
        Some(name.trim().to_string())
    };

    // Host IP address
    let host: String = Input::new()
        .with_prompt("Host IP address")
        .validate_with(|input: &String| -> Result<(), &str> {
            if validate_ip_address(input).is_ok() {
                Ok(())
            } else {
                Err("Invalid IP address format")
            }
        })
        .interact_text()?;

    // SSH port
    let port: u16 = Input::new()
        .with_prompt("SSH port")
        .default(22)
        .validate_with(|input: &u16| -> Result<(), &str> {
            if validate_port(*input).is_ok() {
                Ok(())
            } else {
                Err("Port cannot be 0")
            }
        })
        .interact_text()?;

    // SSH private key path
    let default_key = shellexpand::tilde("~/.ssh/id_rsa").to_string();
    let auth: String = Input::new()
        .with_prompt("SSH private key path")
        .default(default_key)
        .validate_with(|input: &String| -> Result<(), &str> {
            let path = PathBuf::from(shellexpand::tilde(input).to_string());
            if validate_ssh_key_exists(&path).is_ok() {
                Ok(())
            } else {
                Err("SSH key file does not exist")
            }
        })
        .interact_text()?;

    // Root password
    let root_password: String = Password::new()
        .with_prompt("Root password")
        .interact()?;

    // Platform selection
    let platforms = vec!["aurora-arm", "aurora-arm64"];
    let selection = Select::new()
        .with_prompt("Platform")
        .items(&platforms)
        .default(0)
        .interact()?;

    let platform = match selection {
        0 => Platform::AuroraArm,
        1 => Platform::AuroraArm64,
        _ => return Err(anyhow!("Invalid platform selection")),
    };

    // Create device
    let device = Device {
        name,
        host: host.clone(),
        port,
        auth: auth.clone(),
        root_password,
        platform,
        enabled: true,
    };

    // Test connection
    print_info!("Testing SSH connection to {}:{}...", host, port);

    let key_path = device.auth_path();
    let connection_ok = SshClient::test_connection(&host, port, &key_path);

    if !connection_ok {
        println!("\x1b[1m\x1b[93mwarning\x1b[0m: Could not establish SSH connection to the device");

        let add_anyway = Confirm::new()
            .with_prompt("Add device anyway?")
            .default(false)
            .interact()?;

        if !add_anyway {
            return Err(anyhow!("Device not added"));
        }
    } else {
        println!("\x1b[1m\x1b[32msuccess\x1b[0m: SSH connection successful");
    }

    // Save device
    DeviceStore::add(device)?;

    println!("\n\x1b[1m\x1b[32msuccess\x1b[0m: Device added successfully");
    Ok(())
}
