use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::types::{Device, DeviceIdentifier};
use anyhow::Result;
use dialoguer::Confirm;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::Duration;

pub async fn execute(identifier: &str) -> Result<()> {
    let device_id = DeviceIdentifier::parse(identifier);
    let device = DeviceStore::find(&device_id)?;

    println!("\x1b[1mDevice to remove:\x1b[0m");
    println!("  ID: {}", device.id);
    println!("  Name: {}", device.display_name());
    println!("  Host: {}", device.host);
    println!("  Port: {}", device.port);
    println!("  Arch: {}", device.arch);
    println!("  Kind: {}", device.kind);
    if let Some(emulator) = &device.emulator {
        println!("  Overlay: {}", emulator.overlay_image);
        println!("  QMP: {}", emulator.qmp_socket);
        println!("  PID file: {}", emulator.pidfile);
    }

    let prompt = if device.is_emulator() {
        "Remove this emulator device and delete its overlay/runtime artifacts?"
    } else {
        "Are you sure you want to remove this device?"
    };
    let confirmed = Confirm::new()
        .with_prompt(prompt)
        .default(false)
        .interact()?;

    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }

    // Check if this is the currently selected device
    let current_device = DeviceState::get_current().ok();
    let is_current = current_device.as_ref() == Some(&device.id);

    if device.is_emulator() {
        stop_emulator_runtime(&device)?;
        delete_emulator_artifacts(&device)?;
    }

    // Remove device
    DeviceStore::remove(&device_id)?;

    // Clear current device if needed
    if is_current {
        DeviceState::clear_current()?;
        println!("\x1b[1m\x1b[94minfo\x1b[0m: Current device selection cleared");
    }

    println!("\x1b[1m\x1b[32msuccess\x1b[0m: Device removed successfully");
    Ok(())
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

fn delete_emulator_artifacts(device: &Device) -> Result<()> {
    let Some(emulator) = &device.emulator else {
        return Ok(());
    };

    for path in [
        emulator.overlay_image.as_str(),
        emulator.qmp_socket.as_str(),
        emulator.pidfile.as_str(),
    ] {
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
