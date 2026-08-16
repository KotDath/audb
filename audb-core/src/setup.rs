//! Reversible Aurora SDK QEMU wrapper installation.

use crate::config::EmulatorConfig;
use crate::error::{CoreError, CoreResult};
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const TOUCH_PLUGIN: &str =
    "LIPSTICK_LIBINPUT_OPTIONS=-plugin VBoxTouch:qemu:evdev=/dev/input/event1";
const TABLET_IGNORE: &str =
    "ATTRS{name}==\"QEMU Virtio Tablet\", ENV{LIBINPUT_IGNORE_DEVICE}=\"1\"";
const MOUSE_FILES: [(&str, &str); 2] = [
    ("60-emul-wayland-ui.conf", TOUCH_PLUGIN),
    ("99-qemu-touch.rules", TABLET_IGNORE),
];

fn wrapper(socket: &Path, real_binary_name: &str) -> String {
    format!(
        r##"#!/bin/bash
# audb QEMU wrapper - injects QMP and virtual input devices
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ORIG_BIN="${{SCRIPT_DIR}}/{real_binary_name}"
QMP_SOCKET="{}"
mkdir -p "$(dirname "$QMP_SOCKET")"
HAS_QMP=false
HAS_MULTITOUCH=false
HAS_KEYBOARD=false
ARGS=()
for arg in "$@"; do
  case "$arg" in
    -qmp) HAS_QMP=true ;;
    virtio-multitouch-pci) HAS_MULTITOUCH=true ;;
    virtio-keyboard-pci) HAS_KEYBOARD=true ;;
    sdl,gl=on,show-cursor=off) ARGS+=("sdl,gl=on,show-cursor=on"); continue ;;
    sdl,show-cursor=off) ARGS+=("sdl,show-cursor=on"); continue ;;
  esac
  ARGS+=("$arg")
done
if [ "$HAS_QMP" = false ]; then ARGS+=("-qmp" "unix:${{QMP_SOCKET}},server=on,wait=off"); fi
if [ "$HAS_MULTITOUCH" = false ]; then ARGS+=("-device" "virtio-multitouch-pci"); fi
if [ "$HAS_KEYBOARD" = false ]; then ARGS+=("-device" "virtio-keyboard-pci"); fi
exec "$ORIG_BIN" "${{ARGS[@]}}"
"##,
        socket.display()
    )
}

/// Native binary magic bytes: ELF (Linux) and Mach-O thin/fat (macOS).
const BINARY_MAGICS: [[u8; 4]; 7] = [
    *b"\x7fELF",
    [0xFE, 0xED, 0xFA, 0xCE],
    [0xFE, 0xED, 0xFA, 0xCF],
    [0xCE, 0xFA, 0xED, 0xFE],
    [0xCF, 0xFA, 0xED, 0xFE],
    [0xCA, 0xFE, 0xBA, 0xBE],
    [0xCA, 0xFE, 0xBA, 0xBF],
];

fn real_binary_name() -> String {
    format!("{}.real", EmulatorConfig::qemu_binary_name())
}

fn script_marker(path: &Path) -> CoreResult<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut file = fs::File::open(path)?;
    let mut magic = [0_u8; 4];
    file.read_exact(&mut magic)?;
    if BINARY_MAGICS.contains(&magic) {
        return Ok(None);
    }
    Ok(Some(fs::read_to_string(path)?))
}

fn is_our_wrapper(path: &Path) -> CoreResult<bool> {
    Ok(script_marker(path)?
        .is_some_and(|s| s.contains("audb QEMU wrapper") || s.contains("audb2 QEMU wrapper")))
}

fn mouse_path(config: &EmulatorConfig, name: &str) -> std::path::PathBuf {
    config.vmshare().join(name)
}

fn backup_for(path: &Path) -> Option<std::path::PathBuf> {
    ["audb.bak", "audb2.bak"]
        .into_iter()
        .map(|suffix| {
            path.with_extension(format!(
                "{}.{suffix}",
                path.extension()
                    .and_then(|x| x.to_str())
                    .unwrap_or_default()
            ))
        })
        .find(|candidate| candidate.exists())
}

fn new_backup(path: &Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}.audb.bak", path.display()))
}

fn set_mouse_mode(config: &EmulatorConfig) -> CoreResult<bool> {
    let mut changed = false;
    for (name, setting) in MOUSE_FILES {
        let path = mouse_path(config, name);
        if !path.exists() {
            continue;
        }
        if backup_for(&path).is_none() {
            fs::copy(&path, new_backup(&path))?;
        }
        let original = fs::read_to_string(&path)?;
        let mut output = String::new();
        for line in original.lines() {
            if line.trim() == setting {
                output.push_str("# ");
                output.push_str(setting);
                changed = true;
            } else {
                output.push_str(line);
            }
            output.push('\n');
        }
        if output != original {
            fs::write(path, output)?;
        }
    }
    Ok(changed)
}

fn restore_mouse_mode(config: &EmulatorConfig) -> CoreResult<bool> {
    let mut restored = false;
    for (name, _) in MOUSE_FILES {
        let path = mouse_path(config, name);
        if let Some(backup) = backup_for(&path) {
            fs::rename(backup, path)?;
            restored = true;
        }
    }
    Ok(restored)
}

fn mouse_enabled(config: &EmulatorConfig) -> bool {
    MOUSE_FILES.into_iter().all(|(name, setting)| {
        fs::read_to_string(mouse_path(config, name))
            .map(|s| !s.lines().any(|line| line.trim() == setting))
            .unwrap_or(false)
    })
}

pub fn status(config: &EmulatorConfig) -> CoreResult<Value> {
    let installed = config.qemu_real().exists() && is_our_wrapper(&config.qemu_bin())?;
    Ok(json!({
        "installed": installed,
        "qemuBinary": config.qemu_bin(),
        "originalBinary": config.qemu_real(),
        "qmpSocket": config.qmp_socket,
        "pointingDeviceMode": if mouse_enabled(config) { "Mouse" } else { "Touchpad" },
        "config": config.storage_path()?,
    }))
}

pub fn install(config: &EmulatorConfig) -> CoreResult<Value> {
    let qemu = config.qemu_bin();
    let real = config.qemu_real();
    if !qemu.exists() {
        return Err(CoreError::new(
            audb_protocol::ErrorCode::NotFound,
            format!("QEMU binary not found: {}", qemu.display()),
        ));
    }
    let wrapper_status = if is_our_wrapper(&qemu)? {
        if !real.exists() {
            return Err(CoreError::runtime(
                "audb wrapper exists but original QEMU binary is missing",
            ));
        }
        fs::write(&qemu, wrapper(&config.qmp_socket, &real_binary_name()))?;
        fs::set_permissions(&qemu, fs::Permissions::from_mode(0o755))?;
        "updated"
    } else {
        if script_marker(&qemu)?.is_some() {
            return Err(CoreError::runtime(format!(
                "Refusing to replace unknown QEMU wrapper: {}",
                qemu.display()
            )));
        }
        if real.exists() {
            return Err(CoreError::runtime(format!(
                "Refusing to overwrite existing original binary: {}",
                real.display()
            )));
        }
        fs::rename(&qemu, &real)?;
        if let Err(error) = fs::write(&qemu, wrapper(&config.qmp_socket, &real_binary_name())) {
            let _ = fs::rename(&real, &qemu);
            return Err(error.into());
        }
        fs::set_permissions(&qemu, fs::Permissions::from_mode(0o755))?;
        "installed"
    };
    let mouse_changed = set_mouse_mode(config)?;
    if let Some(parent) = config.qmp_socket.parent() {
        fs::create_dir_all(parent)?;
    }
    config.save()?;
    Ok(json!({
        "wrapper": wrapper_status,
        "qemuBinary": qemu,
        "originalBinary": real,
        "qmpSocket": config.qmp_socket,
        "pointingDeviceMode": "Mouse",
        "pointingDeviceChanged": mouse_changed,
        "emulatorStarted": false,
    }))
}

pub fn uninstall(config: &EmulatorConfig) -> CoreResult<Value> {
    let qemu = config.qemu_bin();
    let real = config.qemu_real();
    let restored_mouse = restore_mouse_mode(config)?;
    let restored_wrapper = if real.exists() {
        if qemu.exists() && !is_our_wrapper(&qemu)? {
            return Err(CoreError::runtime(format!(
                "Refusing to remove unknown QEMU wrapper: {}",
                qemu.display()
            )));
        }
        if qemu.exists() {
            fs::remove_file(&qemu)?;
        }
        fs::rename(&real, &qemu)?;
        true
    } else {
        false
    };
    Ok(
        json!({"wrapperRestored": restored_wrapper, "pointingDeviceRestored": restored_mouse, "qemuBinary": qemu}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture() -> (tempfile::TempDir, EmulatorConfig) {
        let dir = tempdir().unwrap();
        let mut config = EmulatorConfig {
            sdk_root: dir.path().into(),
            qmp_socket: dir.path().join("run/qmp.sock"),
            ..EmulatorConfig::default()
        };
        config.use_config_file(dir.path().join("config/emulator.json"));
        fs::create_dir_all(config.qemu_bin().parent().unwrap()).unwrap();
        fs::write(config.qemu_bin(), b"\x7fELFfake").unwrap();
        fs::create_dir_all(config.vmshare()).unwrap();
        for (name, setting) in MOUSE_FILES {
            fs::write(mouse_path(&config, name), format!("{setting}\nother\n")).unwrap();
        }
        (dir, config)
    }

    #[test]
    fn install_and_uninstall_are_reversible() {
        let (_dir, config) = fixture();
        install(&config).unwrap();
        assert!(is_our_wrapper(&config.qemu_bin()).unwrap());
        assert!(config.qemu_real().exists());
        assert!(mouse_enabled(&config));
        uninstall(&config).unwrap();
        assert!(fs::read(config.qemu_bin()).unwrap().starts_with(b"\x7fELF"));
        assert!(!config.qemu_real().exists());
        assert!(!mouse_enabled(&config));
    }

    #[test]
    fn migrates_audb2_wrapper_in_place() {
        let (_dir, config) = fixture();
        fs::rename(config.qemu_bin(), config.qemu_real()).unwrap();
        fs::write(config.qemu_bin(), "#!/bin/bash\n# audb2 QEMU wrapper\n").unwrap();
        install(&config).unwrap();
        assert!(fs::read_to_string(config.qemu_bin())
            .unwrap()
            .contains("/tmp/"));
    }

    #[test]
    fn qemu_binary_matches_host_arch() {
        let expected = format!("qemu-system-{}", std::env::consts::ARCH);
        assert_eq!(EmulatorConfig::qemu_binary_name(), expected);
        let (_dir, config) = fixture();
        assert_eq!(
            config.qemu_bin().file_name().unwrap().to_str().unwrap(),
            expected
        );
        assert_eq!(
            config.qemu_real().file_name().unwrap().to_str().unwrap(),
            format!("{expected}.real")
        );
        install(&config).unwrap();
        let wrapper_script = fs::read_to_string(config.qemu_bin()).unwrap();
        assert!(wrapper_script.contains(&format!("{expected}.real")));
    }

    #[test]
    fn mach_o_binary_is_not_treated_as_script() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("qemu-system-aarch64");
        for magic in [
            [0xFE, 0xED, 0xFA, 0xCF],
            [0xCF, 0xFA, 0xED, 0xFE],
            [0xCA, 0xFE, 0xBA, 0xBE],
        ] {
            fs::write(&path, magic).unwrap();
            assert!(script_marker(&path).unwrap().is_none());
        }
    }
}
