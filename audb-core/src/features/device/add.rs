use crate::features::config::device_store::DeviceStore;
use crate::tools::macros::print_info;
use crate::tools::ssh::SshClient;
use crate::tools::types::{generate_device_id, Device, DeviceArch, DeviceKind};
use crate::tools::validation::{validate_ip_address, validate_port, validate_ssh_key_exists};
use anyhow::{anyhow, Result};
use dialoguer::{Confirm, Input, Password, Select};
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct AddDeviceOptions {
    pub name: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub auth: Option<String>,
    pub root_password: Option<String>,
    pub arch: Option<String>,
    pub add_anyway: bool,
}

pub async fn execute(options: AddDeviceOptions) -> Result<()> {
    println!("\x1b[1m\x1b[36mAdd Aurora OS Device\x1b[0m\n");

    let name = match options.name {
        Some(name) if !name.trim().is_empty() => Some(name.trim().to_string()),
        Some(_) => None,
        None => {
            let name: String = Input::new()
                .with_prompt("Device name (optional, press Enter to skip)")
                .allow_empty(true)
                .interact_text()?;
            if name.trim().is_empty() {
                None
            } else {
                Some(name.trim().to_string())
            }
        }
    };

    let host: String = match options.host {
        Some(host) => {
            validate_ip_address(&host)?;
            host
        }
        None => Input::new()
            .with_prompt("Host IP address")
            .validate_with(|input: &String| -> Result<(), &str> {
                if validate_ip_address(input).is_ok() {
                    Ok(())
                } else {
                    Err("Invalid IP address format")
                }
            })
            .interact_text()?,
    };

    let port: u16 = match options.port {
        Some(port) => {
            validate_port(port)?;
            port
        }
        None => Input::new()
            .with_prompt("SSH port")
            .default(22)
            .validate_with(|input: &u16| -> Result<(), &str> {
                if validate_port(*input).is_ok() {
                    Ok(())
                } else {
                    Err("Port cannot be 0")
                }
            })
            .interact_text()?,
    };

    let default_key = shellexpand::tilde("~/.ssh/id_rsa").to_string();
    let auth: String = match options.auth {
        Some(auth) => {
            let path = PathBuf::from(shellexpand::tilde(&auth).to_string());
            validate_ssh_key_exists(&path)?;
            auth
        }
        None => Input::new()
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
            .interact_text()?,
    };

    let root_password: String = match options.root_password {
        Some(root_password) => root_password,
        None => Password::new()
            .with_prompt("Root password (for devel-su automation - tap/swipe/screenshot)")
            .allow_empty_password(true)
            .interact()?,
    };

    let arch = match options.arch {
        Some(arch) => parse_arch(&arch)?,
        None => {
            let platforms = vec!["aurora-arm", "aurora-arm64"];
            let selection = Select::new()
                .with_prompt("Architecture")
                .items(&platforms)
                .default(0)
                .interact()?;

            match selection {
                0 => DeviceArch::AuroraArm,
                1 => DeviceArch::AuroraArm64,
                _ => return Err(anyhow!("Invalid architecture selection")),
            }
        }
    };

    // Create device
    let device = Device {
        id: generate_device_id(&DeviceKind::Physical),
        name,
        host: host.clone(),
        port,
        auth: auth.clone(),
        root_password: root_password.clone(),
        arch,
        kind: DeviceKind::Physical,
        enabled: true,
        emulator: None,
    };

    // Test defaultuser SSH connection
    print_info("Testing SSH connection as defaultuser...");
    let key_path = device.auth_path();
    let connection_ok = SshClient::test_connection(&host, port, &key_path);

    if !connection_ok {
        println!("\x1b[1m\x1b[93mwarning\x1b[0m: Could not establish SSH connection to the device");

        let add_anyway = options.add_anyway
            || Confirm::new()
                .with_prompt("Add device anyway?")
                .default(false)
                .interact()?;

        if !add_anyway {
            return Err(anyhow!("Device not added"));
        }
    } else {
        println!("\x1b[1m\x1b[32msuccess\x1b[0m: defaultuser SSH connection verified");
    }

    // Save device
    DeviceStore::add(device)?;

    println!("\n\x1b[1m\x1b[32msuccess\x1b[0m: Device added successfully");
    if root_password.is_empty() {
        println!("\x1b[1m\x1b[90mnote\x1b[0m: Tap/swipe/screenshot commands require root password to be configured");
    }
    Ok(())
}

fn parse_arch(arch: &str) -> Result<DeviceArch> {
    match arch {
        "aurora-arm" | "arm" | "armv7hl" => Ok(DeviceArch::AuroraArm),
        "aurora-arm64" | "arm64" | "aarch64" => Ok(DeviceArch::AuroraArm64),
        _ => Err(anyhow!(
            "Unsupported architecture '{}'. Use aurora-arm or aurora-arm64.",
            arch
        )),
    }
}
