use crate::features::config::device_store::DeviceStore;
use crate::tools::types::{generate_device_id, Device, DeviceArch, DeviceKind, QemuEmulatorConfig};
use anyhow::{anyhow, Result};
use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

const DEFAULT_SDK_ROOT: &str = "/home/kotdath/AuroraOS";
const DEFAULT_RELEASE: &str = "AuroraOS-5.2.0.180";
const DEFAULT_SSH_HOST: &str = "127.0.0.1";
const DEFAULT_SSH_PORT_START: u16 = 33223;
const DEFAULT_MEMORY_MB: u32 = 4096;
const DEFAULT_CPUS: u32 = 2;
const DEFAULT_FB_WIDTH: u32 = 360;
const DEFAULT_FB_HEIGHT: u32 = 800;
const DEFAULT_ABS_MAX: u32 = 32767;
const DEFAULT_DISPLAY_PROFILE: &str = "no-gl-hybrid";

pub async fn execute(name: String) -> Result<()> {
    let existing_devices = DeviceStore::list()?;
    if existing_devices
        .iter()
        .any(|device| device.name.as_deref() == Some(name.as_str()))
    {
        return Err(anyhow!("Device with name '{}' already exists", name));
    }
    let device_id = generate_device_id(&DeviceKind::QemuEmulator);
    let sdk_root = PathBuf::from(DEFAULT_SDK_ROOT);
    let base_image = sdk_root
        .join("emulator")
        .join(DEFAULT_RELEASE)
        .join("image.qcow2");
    let emulator_dir = sdk_root.join("emulator").join("audb");
    let overlay_image = emulator_dir.join(format!("{device_id}.qcow2"));
    let ssh_key = sdk_root
        .join("vmshare")
        .join("ssh")
        .join("private_keys")
        .join("sdk");
    let qemu_img = sdk_root
        .join("share")
        .join("qemu")
        .join("bin")
        .join("qemu-img");

    for path in [&base_image, &ssh_key, &qemu_img] {
        if !path.exists() {
            return Err(anyhow!(
                "Required emulator path not found: {}",
                path.display()
            ));
        }
    }

    fs::create_dir_all(&emulator_dir)?;

    if !overlay_image.exists() {
        let status = Command::new(&qemu_img)
            .arg("create")
            .arg("-f")
            .arg("qcow2")
            .arg("-F")
            .arg("qcow2")
            .arg("-b")
            .arg(&base_image)
            .arg(&overlay_image)
            .status()?;
        if !status.success() {
            return Err(anyhow!(
                "Failed to create overlay {} from {}",
                overlay_image.display(),
                base_image.display()
            ));
        }
    }

    let ssh_port = allocate_ssh_port(&existing_devices)?;
    let qmp_socket = format!("/tmp/audb-{device_id}.qmp");
    let pidfile = format!("/tmp/audb-{device_id}.pid");
    let vm_name = format!("audb-{device_id}");
    let mac = derive_mac_address(&device_id);

    let device = Device {
        id: device_id,
        name: Some(name.clone()),
        host: DEFAULT_SSH_HOST.to_string(),
        port: ssh_port,
        auth: ssh_key.display().to_string(),
        root_password: String::new(),
        arch: DeviceArch::AuroraX86_64,
        kind: DeviceKind::QemuEmulator,
        enabled: true,
        emulator: Some(QemuEmulatorConfig {
            sdk_root: sdk_root.display().to_string(),
            release: DEFAULT_RELEASE.to_string(),
            base_image: base_image.display().to_string(),
            overlay_image: overlay_image.display().to_string(),
            ssh_host: DEFAULT_SSH_HOST.to_string(),
            ssh_port,
            ssh_key: ssh_key.display().to_string(),
            qmp_socket: qmp_socket.clone(),
            pidfile: pidfile.clone(),
            vm_name: vm_name.clone(),
            mac: mac.clone(),
            memory_mb: DEFAULT_MEMORY_MB,
            cpus: DEFAULT_CPUS,
            framebuffer_width: DEFAULT_FB_WIDTH,
            framebuffer_height: DEFAULT_FB_HEIGHT,
            abs_max: DEFAULT_ABS_MAX,
            display_profile: DEFAULT_DISPLAY_PROFILE.to_string(),
        }),
    };

    DeviceStore::add(device.clone())?;

    println!("\x1b[1m\x1b[32msuccess\x1b[0m: Emulator created successfully");
    println!("  ID: {}", device.id);
    println!("  Name: {}", device.display_name());
    println!("  Overlay: {}", overlay_image.display());
    println!("  SSH: {}:{}", device.host, device.port);
    println!("  QMP: {}", qmp_socket);
    println!("  PID file: {}", pidfile);
    println!("  VM name: {}", vm_name);
    println!("  MAC: {}", mac);

    Ok(())
}

fn allocate_ssh_port(existing_devices: &[Device]) -> Result<u16> {
    for port in DEFAULT_SSH_PORT_START..u16::MAX {
        let in_config = existing_devices.iter().any(|device| {
            device.is_emulator()
                && device
                    .emulator
                    .as_ref()
                    .map(|config| config.ssh_port == port)
                    .unwrap_or(false)
        });
        if in_config {
            continue;
        }

        if TcpListener::bind((DEFAULT_SSH_HOST, port)).is_ok() {
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
