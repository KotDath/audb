use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::types::{Device, DeviceIdentifier};
use anyhow::{anyhow, Context, Result};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

pub async fn execute(identifier: Option<String>) -> Result<()> {
    let identifier = match identifier {
        Some(identifier) => DeviceIdentifier::parse(&identifier),
        None => DeviceIdentifier::Name(DeviceState::get_current()?),
    };
    let mut device = DeviceStore::find(&identifier)?;
    if !device.is_emulator() {
        return Err(anyhow!(
            "Device '{}' is not an emulator",
            device.display_name()
        ));
    }
    let display_name = device.display_name();

    let existing_devices = DeviceStore::list()?;
    let emulator = device
        .emulator
        .as_ref()
        .ok_or_else(|| anyhow!("Emulator config missing for {}", display_name))?;

    let desired_overlay = PathBuf::from(&emulator.sdk_root)
        .join("emulator")
        .join("audb")
        .join(format!("{}.qcow2", device.id));
    let desired_qmp = format!("/tmp/audb-{}.qmp", device.id);
    let desired_pidfile = format!("/tmp/audb-{}.pid", device.id);
    let desired_vm_name = format!("audb-{}", device.id);
    let desired_mac = derive_mac_address(&device.id);
    let desired_port = if port_conflicts(&existing_devices, &device.id, emulator.ssh_port) {
        allocate_ssh_port(&existing_devices, &device.id)?
    } else {
        emulator.ssh_port
    };

    let already_migrated = Path::new(&emulator.overlay_image) == desired_overlay
        && emulator.qmp_socket == desired_qmp
        && emulator.pidfile == desired_pidfile
        && emulator.vm_name == desired_vm_name
        && emulator.mac == desired_mac
        && emulator.ssh_port == desired_port
        && device.port == desired_port;

    if already_migrated {
        println!(
            "\x1b[1m\x1b[32msuccess\x1b[0m: Emulator {} is already using per-device runtime paths",
            device.display_name()
        );
        return Ok(());
    }

    stop_emulator_runtime(&device)?;

    let old_overlay = PathBuf::from(&emulator.overlay_image);
    if !old_overlay.exists() && !desired_overlay.exists() {
        return Err(anyhow!(
            "Neither legacy overlay {} nor target overlay {} exists",
            old_overlay.display(),
            desired_overlay.display()
        ));
    }

    if old_overlay != desired_overlay {
        if desired_overlay.exists() {
            return Err(anyhow!(
                "Target overlay already exists: {}",
                desired_overlay.display()
            ));
        }
        if let Some(parent) = desired_overlay.parent() {
            fs::create_dir_all(parent)?;
        }
        if old_overlay.exists() {
            fs::rename(&old_overlay, &desired_overlay).or_else(|_| {
                fs::copy(&old_overlay, &desired_overlay)
                    .map(|_| ())
                    .and_then(|_| fs::remove_file(&old_overlay))
            })?;
        }
    }

    cleanup_runtime_artifacts([emulator.qmp_socket.as_str(), emulator.pidfile.as_str()])?;

    {
        let emulator = device
            .emulator
            .as_mut()
            .ok_or_else(|| anyhow!("Emulator config missing for {}", display_name))?;
        emulator.overlay_image = desired_overlay.display().to_string();
        emulator.qmp_socket = desired_qmp.clone();
        emulator.pidfile = desired_pidfile.clone();
        emulator.vm_name = desired_vm_name.clone();
        emulator.mac = desired_mac.clone();
        emulator.ssh_port = desired_port;
    }
    device.port = desired_port;

    DeviceStore::update(device.clone())
        .with_context(|| format!("Failed to update emulator {}", display_name))?;

    println!(
        "\x1b[1m\x1b[32msuccess\x1b[0m: Migrated emulator {} to per-device runtime layout",
        display_name
    );
    println!("  Overlay: {}", desired_overlay.display());
    println!("  SSH: {}:{}", device.host, device.port);
    println!("  QMP: {}", desired_qmp);
    println!("  PID file: {}", desired_pidfile);
    println!("  VM name: {}", desired_vm_name);
    println!("  MAC: {}", desired_mac);

    Ok(())
}

fn port_conflicts(devices: &[Device], current_device_id: &str, port: u16) -> bool {
    devices.iter().any(|device| {
        device.id != current_device_id
            && device.is_emulator()
            && device
                .emulator
                .as_ref()
                .map(|config| config.ssh_port == port)
                .unwrap_or(false)
    })
}

fn allocate_ssh_port(devices: &[Device], current_device_id: &str) -> Result<u16> {
    for port in 33223..u16::MAX {
        let in_config = devices.iter().any(|device| {
            device.id != current_device_id
                && device.is_emulator()
                && device
                    .emulator
                    .as_ref()
                    .map(|config| config.ssh_port == port)
                    .unwrap_or(false)
        });
        if in_config {
            continue;
        }

        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }

    Err(anyhow!("Unable to allocate a free SSH port for emulator"))
}

fn derive_mac_address(device_id: &str) -> String {
    let hex: String = device_id
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect();
    let tail = if hex.len() > 8 {
        hex[hex.len() - 8..].to_string()
    } else {
        hex
    };
    let padded = format!("{tail:0>8}");
    let bytes = (0..4)
        .map(|idx| u8::from_str_radix(&padded[idx * 2..idx * 2 + 2], 16).unwrap_or(0))
        .collect::<Vec<_>>();

    format!(
        "52:54:{:02x}:{:02x}:{:02x}:{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3]
    )
}

fn stop_emulator_runtime(device: &Device) -> Result<()> {
    let Some(emulator) = &device.emulator else {
        return Ok(());
    };

    if let Some(pid) = read_pidfile(&emulator.pidfile)? {
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        for _ in 0..40 {
            if !pid_exists(pid) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        if pid_exists(pid) {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }

    Ok(())
}

fn cleanup_runtime_artifacts<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<()> {
    for path in paths {
        let path = Path::new(path);
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn read_pidfile(path: &str) -> Result<Option<i32>> {
    let path = Path::new(path);
    if !path.exists() {
        return Ok(None);
    }

    let pid = fs::read_to_string(path)?.trim().parse::<i32>()?;
    Ok(Some(pid))
}

fn pid_exists(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}
