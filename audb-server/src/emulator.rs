use anyhow::{anyhow, Context, Result};
use audb_core::tools::ssh::SshClient;
use audb_core::tools::types::{Device, DeviceKind, QemuEmulatorConfig};
use audb_protocol::{
    DisplayGeometryInfo, EmulatorLifecycleStateInfo, EmulatorStatus, ScreenOrientationInfo,
    SwipeDirection, SwipeMode,
};
use image::imageops::FilterType;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use crate::pool::ConnectionPool;

const DEFAULT_TAP_HOLD_MS: u32 = 90;
const DEFAULT_TAP_SETTLE_MS: u64 = 400;
const DEFAULT_SWIPE_STEPS: u32 = 42;
const DEFAULT_SWIPE_DURATION_MS: u32 = 900;
const DEFAULT_SWIPE_HOLD_MS: u32 = 160;
const DEFAULT_VERTICAL_SWIPE_SETTLE_MS: u64 = 3000;
const DEFAULT_HORIZONTAL_SWIPE_SETTLE_MS: u64 = 450;
const SCROLL_START_Y_RATIO: f64 = 0.78;
const SCROLL_END_Y_RATIO: f64 = 0.22;
const EDGE_START_Y_RATIO: f64 = 0.985;
const EDGE_END_Y_RATIO: f64 = 0.06;
const KEYBOARD_PASTE_TAP_HOLD_MS: u32 = 160;
const KEYBOARD_PORTRAIT_ROWS_WITH_HANDLER: f64 = 5.0;

#[derive(Clone, Default)]
struct ManagedEmulatorState {
    lifecycle: ManagedLifecycle,
    cached_geometry: Option<CachedGeometry>,
}

#[derive(Clone, Default)]
enum ManagedLifecycle {
    #[default]
    Stopped,
    Starting,
    Running,
    Errored,
}

#[derive(Clone)]
struct CachedGeometry {
    native_width: u32,
    native_height: u32,
}

#[derive(Clone, Copy)]
struct ResolvedGeometry {
    native_width: u32,
    native_height: u32,
    visible_width: u32,
    visible_height: u32,
    orientation: Orientation,
    abs_max: u32,
}

#[derive(Clone, Copy)]
enum Orientation {
    Portrait,
    Landscape,
    InvertedPortrait,
    InvertedLandscape,
}

pub struct EmulatorManager {
    states: Arc<Mutex<HashMap<String, ManagedEmulatorState>>>,
}

impl EmulatorManager {
    pub fn new() -> Self {
        Self {
            states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn ensure_runtime(&self, device: &Device, pool: &ConnectionPool) -> Result<()> {
        if !device.is_emulator() {
            return Ok(());
        }

        let status = self.status(device, pool).await?;
        if matches!(status.lifecycle, EmulatorLifecycleStateInfo::Running)
            && status.ssh_ready
            && status.qmp_ready
        {
            return Ok(());
        }

        self.start(device, pool).await.map(|_| ())
    }

    pub async fn start(&self, device: &Device, pool: &ConnectionPool) -> Result<EmulatorStatus> {
        let config = emulator_config(device)?;

        self.set_lifecycle(&device.id, ManagedLifecycle::Starting)
            .await;
        self.invalidate_geometry(&device.id).await;

        cleanup_runtime_artifacts(config)?;

        let qemu_bin = PathBuf::from(&config.sdk_root)
            .join("share")
            .join("qemu")
            .join("bin")
            .join("qemu-system-x86_64");
        let vm_dir = PathBuf::from(&config.sdk_root)
            .join("emulator")
            .join(&config.release);
        let configs_dir = vm_dir.join("vmshare");
        let media_dir = PathBuf::from(&config.sdk_root)
            .join("emulator")
            .join("media");
        let ssh_dir = vm_dir.join("ssh");

        for path in [
            &qemu_bin,
            Path::new(&config.base_image),
            Path::new(&config.overlay_image),
            &configs_dir,
            &media_dir,
            &ssh_dir,
        ] {
            if !path.exists() {
                self.set_lifecycle(&device.id, ManagedLifecycle::Errored)
                    .await;
                return Err(anyhow!(
                    "Required emulator path is missing: {}",
                    path.display()
                ));
            }
        }

        let args = vec![
            "-M".to_string(),
            "q35,i8042=off".to_string(),
            "-cpu".to_string(),
            "host".to_string(),
            "-m".to_string(),
            format!("{}M", config.memory_mb),
            "-smp".to_string(),
            config.cpus.to_string(),
            "--enable-kvm".to_string(),
            "-name".to_string(),
            config.vm_name.clone(),
            "-device".to_string(),
            format!(
                "virtio-vga,xres={},yres={}",
                config.framebuffer_width, config.framebuffer_height
            ),
            "-display".to_string(),
            "sdl,show-cursor=off".to_string(),
            "-device".to_string(),
            "virtio-tablet-pci".to_string(),
            "-device".to_string(),
            "virtio-multitouch-pci".to_string(),
            "-device".to_string(),
            "ahci,id=ahci".to_string(),
            "-device".to_string(),
            "ide-hd,drive=disk,bus=ahci.0".to_string(),
            "-audiodev".to_string(),
            "sdl,id=audiodev0".to_string(),
            "-device".to_string(),
            "intel-hda".to_string(),
            "-device".to_string(),
            "hda-output,audiodev=audiodev0".to_string(),
            "-nodefaults".to_string(),
            "-drive".to_string(),
            format!("id=disk,file={},if=none", config.overlay_image),
            "-nic".to_string(),
            format!(
                "user,mac={},hostfwd=tcp::{}-:22",
                config.mac, config.ssh_port
            ),
            "-virtfs".to_string(),
            format!(
                "local,path={},mount_tag=configs,security_model=mapped,readonly=on",
                configs_dir.display()
            ),
            "-virtfs".to_string(),
            format!(
                "local,path={},mount_tag=media,security_model=mapped,readonly=on",
                media_dir.display()
            ),
            "-virtfs".to_string(),
            format!(
                "local,path={},mount_tag=ssh,security_model=mapped,readonly=on",
                ssh_dir.display()
            ),
            "-qmp".to_string(),
            format!("unix:{},server=on,wait=off", config.qmp_socket),
            "-pidfile".to_string(),
            config.pidfile.clone(),
        ];

        let mut command = ProcessCommand::new(&qemu_bin);
        command
            .env("LIBGL_ALWAYS_SOFTWARE", "1")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = command.spawn().context("Failed to spawn QEMU emulator")?;
        drop(child);

        for _ in 0..120 {
            if let Ok(status) = self.status(device, pool).await {
                if matches!(status.lifecycle, EmulatorLifecycleStateInfo::Running)
                    && status.ssh_ready
                    && status.qmp_ready
                {
                    self.set_lifecycle(&device.id, ManagedLifecycle::Running)
                        .await;
                    return Ok(status);
                }
            }

            if let Some(pid) = read_pidfile(&config.pidfile)? {
                if !pid_exists(pid) {
                    break;
                }
            }

            tokio::time::sleep(Duration::from_millis(250)).await;
        }

        self.set_lifecycle(&device.id, ManagedLifecycle::Errored)
            .await;
        Err(anyhow!("Emulator failed to reach ready state"))
    }

    pub async fn stop(&self, device: &Device) -> Result<EmulatorStatus> {
        let config = emulator_config(device)?;
        if let Some(pid) = read_pidfile(&config.pidfile)? {
            kill(Pid::from_raw(pid), Signal::SIGTERM).ok();
            for _ in 0..40 {
                if !pid_exists(pid) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if pid_exists(pid) {
                kill(Pid::from_raw(pid), Signal::SIGKILL).ok();
            }
        }
        cleanup_runtime_artifacts(config)?;
        self.invalidate_geometry(&device.id).await;
        self.set_lifecycle(&device.id, ManagedLifecycle::Stopped)
            .await;
        Ok(EmulatorStatus {
            lifecycle: EmulatorLifecycleStateInfo::Stopped,
            ssh_ready: false,
            qmp_ready: false,
            qmp_input_ready: false,
            qmp_screendump_ready: false,
            geometry: None,
        })
    }

    pub async fn reconnect(&self, device: &Device) {
        self.invalidate_geometry(&device.id).await;
        let lifecycle = if is_running(device) {
            ManagedLifecycle::Running
        } else {
            ManagedLifecycle::Stopped
        };
        self.set_lifecycle(&device.id, lifecycle).await;
    }

    pub async fn status(&self, device: &Device, pool: &ConnectionPool) -> Result<EmulatorStatus> {
        let config = emulator_config(device)?;
        let lifecycle = self.current_lifecycle(device).await;
        let ssh_ready = SshClient::test_connection(&device.host, device.port, &device.auth_path());

        let qmp_details = query_qmp_capabilities(&config.qmp_socket).ok();
        let qmp_ready = qmp_details.is_some();
        let qmp_input_ready = qmp_details
            .as_ref()
            .map(|caps| caps.input_send_event)
            .unwrap_or(false);
        let qmp_screendump_ready = qmp_details
            .as_ref()
            .map(|caps| caps.screendump && config.display_profile == "no-gl-hybrid")
            .unwrap_or(false);

        let geometry = if ssh_ready {
            self.resolve_geometry(device, pool)
                .await
                .ok()
                .map(|geometry| DisplayGeometryInfo {
                    native_width: geometry.native_width,
                    native_height: geometry.native_height,
                    visible_width: geometry.visible_width,
                    visible_height: geometry.visible_height,
                    orientation: geometry.orientation.into(),
                })
        } else {
            None
        };

        Ok(EmulatorStatus {
            lifecycle,
            ssh_ready,
            qmp_ready,
            qmp_input_ready,
            qmp_screendump_ready,
            geometry,
        })
    }

    pub async fn tap(
        &self,
        device: &Device,
        pool: &ConnectionPool,
        x: u16,
        y: u16,
        duration_ms: Option<u32>,
    ) -> Result<Vec<String>> {
        self.ensure_runtime(device, pool).await?;
        let geometry = self.resolve_geometry(device, pool).await?;
        let (abs_x, abs_y) = geometry.pixel_to_abs(x as u32, y as u32)?;
        qmp_send_events(
            &emulator_config(device)?.qmp_socket,
            vec![
                qmp_mtt("begin", 0, 1001, "x", abs_x),
                qmp_btn_touch(true),
                qmp_mtt("data", 0, 1001, "x", abs_x),
                qmp_mtt("data", 0, 1001, "y", abs_y),
            ],
        )?;
        std::thread::sleep(Duration::from_millis(
            duration_ms.unwrap_or(DEFAULT_TAP_HOLD_MS) as u64,
        ));
        qmp_send_events(
            &emulator_config(device)?.qmp_socket,
            vec![qmp_mtt("end", 0, -1, "x", abs_x)],
        )?;
        std::thread::sleep(Duration::from_millis(DEFAULT_TAP_SETTLE_MS));
        Ok(vec![format!("tap({}, {}) via QMP multitouch", x, y)])
    }

    pub async fn swipe(
        &self,
        device: &Device,
        pool: &ConnectionPool,
        mode: SwipeMode,
        steps: Option<u32>,
        duration_ms: Option<u32>,
        hold_ms: Option<u32>,
    ) -> Result<Vec<String>> {
        self.ensure_runtime(device, pool).await?;
        let geometry = self.resolve_geometry(device, pool).await?;
        let (x1, y1, x2, y2) = swipe_coords(mode, geometry)?;
        let (start_x, start_y) = geometry.pixel_to_abs(x1, y1)?;
        let (end_x, end_y) = geometry.pixel_to_abs(x2, y2)?;
        let steps = steps.unwrap_or(DEFAULT_SWIPE_STEPS).max(1);
        let duration_ms = duration_ms.unwrap_or(DEFAULT_SWIPE_DURATION_MS);
        let hold_ms = hold_ms.unwrap_or(DEFAULT_SWIPE_HOLD_MS);
        let settle_ms = if y1.abs_diff(y2) >= x1.abs_diff(x2) {
            DEFAULT_VERTICAL_SWIPE_SETTLE_MS
        } else {
            DEFAULT_HORIZONTAL_SWIPE_SETTLE_MS
        };
        let delay = Duration::from_millis((duration_ms / steps) as u64);
        let socket = emulator_config(device)?.qmp_socket.clone();

        qmp_send_events(
            &socket,
            vec![
                qmp_mtt("begin", 0, 1001, "x", start_x),
                qmp_btn_touch(true),
                qmp_mtt("data", 0, 1001, "x", start_x),
                qmp_mtt("data", 0, 1001, "y", start_y),
            ],
        )?;
        std::thread::sleep(Duration::from_millis(hold_ms as u64));

        for step in 1..=steps {
            let progress = step as f64 / steps as f64;
            let cur_x = lerp(start_x, end_x, progress);
            let cur_y = lerp(start_y, end_y, progress);
            qmp_send_events(
                &socket,
                vec![
                    qmp_mtt("update", 0, 1001, "x", cur_x),
                    qmp_btn_touch(true),
                    qmp_mtt("data", 0, 1001, "x", cur_x),
                    qmp_mtt("data", 0, 1001, "y", cur_y),
                ],
            )?;
            std::thread::sleep(delay);
        }

        qmp_send_events(&socket, vec![qmp_mtt("end", 0, -1, "x", end_x)])?;
        std::thread::sleep(Duration::from_millis(settle_ms));
        Ok(vec!["swipe via QMP multitouch".to_string()])
    }

    pub async fn paste_clipboard(
        &self,
        device: &Device,
        pool: &ConnectionPool,
    ) -> Result<Vec<String>> {
        self.ensure_runtime(device, pool).await?;
        let geometry = self.resolve_geometry(device, pool).await?;
        let keyboard_height = query_keyboard_height(pool, &device.id).await?;
        let (x, y) = geometry.clipboard_button_center(keyboard_height)?;
        let (abs_x, abs_y) = geometry.pixel_to_abs(x, y)?;

        qmp_send_events(
            &emulator_config(device)?.qmp_socket,
            vec![
                qmp_mtt("begin", 0, 1001, "x", abs_x),
                qmp_btn_touch(true),
                qmp_mtt("data", 0, 1001, "x", abs_x),
                qmp_mtt("data", 0, 1001, "y", abs_y),
            ],
        )?;
        std::thread::sleep(Duration::from_millis(KEYBOARD_PASTE_TAP_HOLD_MS as u64));
        qmp_send_events(
            &emulator_config(device)?.qmp_socket,
            vec![qmp_mtt("end", 0, -1, "x", abs_x)],
        )?;
        std::thread::sleep(Duration::from_millis(DEFAULT_TAP_SETTLE_MS));

        Ok(vec![format!(
            "clipboard button tapped at ({}, {}) via QMP multitouch",
            x, y
        )])
    }

    pub async fn screenshot(&self, device: &Device, pool: &ConnectionPool) -> Result<Vec<u8>> {
        self.ensure_runtime(device, pool).await?;
        let config = emulator_config(device)?;
        if config.display_profile != "no-gl-hybrid" {
            return Err(anyhow!(
                "QMP screendump is only supported for no-gl-hybrid emulator profile"
            ));
        }

        let output = std::env::temp_dir().join(format!("audb-{}-screenshot.png", device.id));
        if output.exists() {
            fs::remove_file(&output).ok();
        }
        qmp_execute(
            &config.qmp_socket,
            "screendump",
            Some(json!({
                "filename": output.display().to_string(),
                "format": "png"
            })),
        )?;
        let raw = fs::read(&output)
            .with_context(|| format!("Failed to read QMP screendump {}", output.display()))?;
        fs::remove_file(&output).ok();
        let geometry = self.resolve_geometry(device, pool).await?;
        normalize_screenshot_dimensions(raw, geometry.visible_width, geometry.visible_height)
    }

    pub async fn key(
        &self,
        device: &Device,
        pool: &ConnectionPool,
        key_name: &str,
    ) -> Result<Vec<String>> {
        self.ensure_runtime(device, pool).await?;
        let key = key_name.to_lowercase();
        match key.as_str() {
            "power" => {
                pool.execute_command(
                    &device.id,
                    "gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.req_trigger_powerkey_event 0",
                    false,
                )
                .await?;
                Ok(vec!["key 'power' via mce".to_string()])
            }
            "lock" => {
                pool.execute_command(
                    &device.id,
                    "gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.req_tklock_mode_change 'locked'",
                    false,
                )
                .await?;
                Ok(vec!["key 'lock' via mce".to_string()])
            }
            "unlock" => {
                pool.execute_command(
                    &device.id,
                    "gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.req_tklock_mode_change 'unlocked'",
                    false,
                )
                .await?;
                pool.execute_command(
                    &device.id,
                    "gdbus call --system --dest com.nokia.mce --object-path /com/nokia/mce/request --method com.nokia.mce.request.req_display_state_on",
                    false,
                )
                .await?;
                Ok(vec!["key 'unlock' via mce".to_string()])
            }
            "volumeup" | "vol+" => {
                qmp_send_events(
                    &emulator_config(device)?.qmp_socket,
                    vec![qmp_key("volumeup", true), qmp_key("volumeup", false)],
                )?;
                Ok(vec!["key 'volumeup' via QMP keyboard".to_string()])
            }
            "volumedown" | "vol-" => {
                qmp_send_events(
                    &emulator_config(device)?.qmp_socket,
                    vec![qmp_key("volumedown", true), qmp_key("volumedown", false)],
                )?;
                Ok(vec!["key 'volumedown' via QMP keyboard".to_string()])
            }
            "home" | "close" => {
                self.swipe(
                    device,
                    pool,
                    SwipeMode::Direction(SwipeDirection::LongUp),
                    Some(DEFAULT_SWIPE_STEPS),
                    Some(DEFAULT_SWIPE_DURATION_MS),
                    Some(DEFAULT_SWIPE_HOLD_MS),
                )
                .await
            }
            "back" => {
                self.swipe(
                    device,
                    pool,
                    SwipeMode::Direction(SwipeDirection::Right),
                    Some(28),
                    Some(420),
                    Some(90),
                )
                .await
            }
            "menu" => {
                self.swipe(
                    device,
                    pool,
                    SwipeMode::Direction(SwipeDirection::LongDown),
                    Some(28),
                    Some(420),
                    Some(90),
                )
                .await
            }
            _ => Err(anyhow!("Unsupported emulator key '{}'", key_name)),
        }
    }

    async fn resolve_geometry(
        &self,
        device: &Device,
        pool: &ConnectionPool,
    ) -> Result<ResolvedGeometry> {
        let config = emulator_config(device)?;
        let native = if let Some(geometry) = self.cached_geometry(&device.id).await {
            geometry
        } else {
            let geometry = query_native_geometry(pool, &device.id).await?;
            self.cache_geometry(&device.id, geometry.clone()).await;
            geometry
        };

        let orientation = query_orientation(pool, &device.id).await?;
        let (visible_width, visible_height) = match orientation {
            Orientation::Portrait | Orientation::InvertedPortrait => {
                (native.native_width, native.native_height)
            }
            Orientation::Landscape | Orientation::InvertedLandscape => {
                (native.native_height, native.native_width)
            }
        };

        Ok(ResolvedGeometry {
            native_width: native.native_width,
            native_height: native.native_height,
            visible_width,
            visible_height,
            orientation,
            abs_max: config.abs_max,
        })
    }

    async fn current_lifecycle(&self, device: &Device) -> EmulatorLifecycleStateInfo {
        let managed = {
            let states = self.states.lock().await;
            states.get(&device.id).cloned().unwrap_or_default()
        };

        if is_running(device) {
            return EmulatorLifecycleStateInfo::Running;
        }

        match managed.lifecycle {
            ManagedLifecycle::Stopped => EmulatorLifecycleStateInfo::Stopped,
            ManagedLifecycle::Starting => EmulatorLifecycleStateInfo::Starting,
            ManagedLifecycle::Running => EmulatorLifecycleStateInfo::Running,
            ManagedLifecycle::Errored => EmulatorLifecycleStateInfo::Errored,
        }
    }

    async fn cached_geometry(&self, device_id: &str) -> Option<CachedGeometry> {
        let states = self.states.lock().await;
        states
            .get(device_id)
            .and_then(|state| state.cached_geometry.clone())
    }

    async fn cache_geometry(&self, device_id: &str, geometry: CachedGeometry) {
        let mut states = self.states.lock().await;
        let state = states.entry(device_id.to_string()).or_default();
        state.cached_geometry = Some(geometry);
    }

    async fn invalidate_geometry(&self, device_id: &str) {
        let mut states = self.states.lock().await;
        let state = states.entry(device_id.to_string()).or_default();
        state.cached_geometry = None;
    }

    async fn set_lifecycle(&self, device_id: &str, lifecycle: ManagedLifecycle) {
        let mut states = self.states.lock().await;
        let state = states.entry(device_id.to_string()).or_default();
        state.lifecycle = lifecycle;
    }
}

impl Default for EmulatorManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolvedGeometry {
    fn pixel_to_abs(&self, x: u32, y: u32) -> Result<(u32, u32)> {
        if x >= self.visible_width || y >= self.visible_height {
            return Err(anyhow!(
                "Coordinates out of range: ({}, {}) for visible geometry {}x{}",
                x,
                y,
                self.visible_width,
                self.visible_height
            ));
        }

        let (native_x, native_y) = match self.orientation {
            Orientation::Portrait => (x, y),
            Orientation::Landscape => (self.native_width - 1 - y, x),
            Orientation::InvertedPortrait => {
                (self.native_width - 1 - x, self.native_height - 1 - y)
            }
            Orientation::InvertedLandscape => (y, self.native_height - 1 - x),
        };

        let abs_x = ((native_x as f64) * self.abs_max as f64 / (self.native_width - 1) as f64)
            .round() as u32;
        let abs_y = ((native_y as f64) * self.abs_max as f64 / (self.native_height - 1) as f64)
            .round() as u32;
        Ok((abs_x, abs_y))
    }

    fn clipboard_button_center(&self, keyboard_height: f64) -> Result<(u32, u32)> {
        if keyboard_height <= 0.0 {
            return Err(anyhow!(
                "keyboard height is zero; focus a text field and open the keyboard before paste"
            ));
        }

        let keyboard_height = keyboard_height.min(self.visible_height as f64);
        let key_height = keyboard_height / KEYBOARD_PORTRAIT_ROWS_WITH_HANDLER;
        let x = (key_height / 2.0)
            .round()
            .clamp(0.0, (self.visible_width - 1) as f64) as u32;
        let y = ((self.visible_height as f64 - keyboard_height) + key_height / 2.0)
            .round()
            .clamp(0.0, (self.visible_height - 1) as f64) as u32;

        Ok((x, y))
    }
}

impl From<Orientation> for ScreenOrientationInfo {
    fn from(value: Orientation) -> Self {
        match value {
            Orientation::Portrait => ScreenOrientationInfo::Portrait,
            Orientation::Landscape => ScreenOrientationInfo::Landscape,
            Orientation::InvertedPortrait => ScreenOrientationInfo::InvertedPortrait,
            Orientation::InvertedLandscape => ScreenOrientationInfo::InvertedLandscape,
        }
    }
}

fn emulator_config(device: &Device) -> Result<&QemuEmulatorConfig> {
    if device.kind != DeviceKind::QemuEmulator {
        return Err(anyhow!("Device {} is not an emulator", device.id));
    }
    device
        .emulator
        .as_ref()
        .ok_or_else(|| anyhow!("Emulator config missing for device {}", device.id))
}

fn is_running(device: &Device) -> bool {
    device
        .emulator
        .as_ref()
        .and_then(|config| read_pidfile(&config.pidfile).ok().flatten())
        .map(pid_exists)
        .unwrap_or(false)
}

fn cleanup_runtime_artifacts(config: &QemuEmulatorConfig) -> Result<()> {
    for path in [&config.qmp_socket, &config.pidfile] {
        let path = Path::new(path);
        if path.exists() {
            fs::remove_file(path).ok();
        }
    }
    Ok(())
}

fn read_pidfile(path: &str) -> Result<Option<i32>> {
    let path = Path::new(path);
    if !path.exists() {
        return Ok(None);
    }
    let pid = fs::read_to_string(path)?
        .trim()
        .parse::<i32>()
        .with_context(|| format!("Invalid pid in {}", path.display()))?;
    Ok(Some(pid))
}

fn pid_exists(pid: i32) -> bool {
    PathBuf::from(format!("/proc/{}", pid)).exists()
}

#[derive(Default)]
struct QmpCapabilityDetails {
    input_send_event: bool,
    screendump: bool,
}

fn query_qmp_capabilities(socket_path: &str) -> Result<QmpCapabilityDetails> {
    let commands = qmp_execute(socket_path, "query-commands", None)?;
    let mut result = QmpCapabilityDetails::default();
    let items = commands
        .get("return")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("query-commands returned unexpected payload"))?;
    for item in items {
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            match name {
                "input-send-event" => result.input_send_event = true,
                "screendump" => result.screendump = true,
                _ => {}
            }
        }
    }
    Ok(result)
}

async fn query_native_geometry(pool: &ConnectionPool, device_id: &str) -> Result<CachedGeometry> {
    let screen_grab_output = pool
        .execute_command(
            device_id,
            "dbus-send --session --print-reply --dest=ru.auroraos.ScreenGrab1.Backend /ru/auroraos/ScreenGrab1/Backend ru.auroraos.ScreenGrab1.Backend.GetScreenInfo",
            false,
        )
        .await
        .unwrap_or_default()
        .join(" ");

    if let Some((width, height)) = parse_dimensions_from_text(&screen_grab_output) {
        return Ok(CachedGeometry {
            native_width: width,
            native_height: height,
        });
    }

    let resolution_output = pool
        .execute_command(
            device_id,
            "gdbus call --system --dest ru.omp.deviceinfo --object-path /ru/omp/deviceinfo/Features --method ru.omp.deviceinfo.Features.getScreenResolution",
            false,
        )
        .await?;
    let resolution_text = resolution_output.join(" ");
    if let Some((width, height)) = parse_dimensions_from_text(&resolution_text) {
        return Ok(CachedGeometry {
            native_width: width,
            native_height: height,
        });
    }

    Err(anyhow!("failed to resolve emulator display geometry"))
}

async fn query_orientation(pool: &ConnectionPool, device_id: &str) -> Result<Orientation> {
    let output = pool
        .execute_command(
            device_id,
            "dconf read /desktop/lipstick-jolla-home/dialog_orientation",
            false,
        )
        .await?;
    let raw = output.join("").trim().parse::<i32>().unwrap_or(1);
    let orientation = match raw {
        2 => Orientation::Landscape,
        4 => Orientation::InvertedPortrait,
        8 => Orientation::InvertedLandscape,
        _ => Orientation::Portrait,
    };
    Ok(orientation)
}

async fn query_keyboard_height(pool: &ConnectionPool, device_id: &str) -> Result<f64> {
    let output = pool
        .execute_command(
            device_id,
            "gdbus call --session --dest org.maliit.server --object-path /com/jolla/keyboard --method org.freedesktop.DBus.Properties.Get com.jolla.keyboard keyboardHeight",
            false,
        )
        .await?;
    let text = output.join(" ");
    parse_first_float(&text).ok_or_else(|| anyhow!("failed to parse keyboard height from {}", text))
}

fn parse_dimensions_from_text(text: &str) -> Option<(u32, u32)> {
    let int32_values: Vec<u32> = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .filter_map(|window| match window {
            ["int32", value] => value.parse::<u32>().ok(),
            _ => None,
        })
        .collect();
    if let [width, height, ..] = int32_values.as_slice() {
        if *width >= 100 && *height >= 100 {
            return Some((*width, *height));
        }
    }

    for token in text.split(|c: char| c.is_whitespace() || ",;()[]{}<>\"'".contains(c)) {
        if let Some((width, height)) = token.split_once('x') {
            if let (Ok(width), Ok(height)) = (width.parse::<u32>(), height.parse::<u32>()) {
                if width > 0 && height > 0 {
                    return Some((width, height));
                }
            }
        }
    }

    let mut ints = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            if let Ok(value) = current.parse::<u32>() {
                ints.push(value);
            }
            current.clear();
        }
    }
    if !current.is_empty() {
        if let Ok(value) = current.parse::<u32>() {
            ints.push(value);
        }
    }

    ints.windows(2).find_map(|pair| match pair {
        [width, height] if *width >= 100 && *height >= 100 => Some((*width, *height)),
        _ => None,
    })
}

fn parse_first_float(text: &str) -> Option<f64> {
    text.split(|ch: char| !(ch.is_ascii_digit() || ch == '.' || ch == '-'))
        .filter(|part| !part.is_empty())
        .find_map(|part| part.parse::<f64>().ok())
}

fn swipe_coords(mode: SwipeMode, geometry: ResolvedGeometry) -> Result<(u32, u32, u32, u32)> {
    match mode {
        SwipeMode::Coords { x1, y1, x2, y2 } => Ok((x1 as u32, y1 as u32, x2 as u32, y2 as u32)),
        SwipeMode::Direction(direction) => {
            let width = geometry.visible_width;
            let height = geometry.visible_height;
            let center_x = width / 2;
            let center_y = height / 2;
            let margin_x = (width / 10).max(20);
            match direction {
                SwipeDirection::Up => Ok((
                    center_x,
                    ratio_coord(height, SCROLL_START_Y_RATIO),
                    center_x,
                    ratio_coord(height, SCROLL_END_Y_RATIO),
                )),
                SwipeDirection::Down => Ok((
                    center_x,
                    ratio_coord(height, SCROLL_END_Y_RATIO),
                    center_x,
                    ratio_coord(height, SCROLL_START_Y_RATIO),
                )),
                SwipeDirection::LongUp => Ok((
                    center_x,
                    ratio_coord(height, EDGE_START_Y_RATIO),
                    center_x,
                    ratio_coord(height, EDGE_END_Y_RATIO),
                )),
                SwipeDirection::LongDown => Ok((
                    center_x,
                    1,
                    center_x,
                    ratio_coord(height, 1.0 - EDGE_END_Y_RATIO),
                )),
                SwipeDirection::Left => Ok((width - margin_x, center_y, margin_x, center_y)),
                SwipeDirection::Right => Ok((margin_x, center_y, width - margin_x, center_y)),
            }
        }
    }
}

fn lerp(start: u32, end: u32, progress: f64) -> u32 {
    (start as f64 + (end as f64 - start as f64) * progress).round() as u32
}

fn ratio_coord(size: u32, ratio: f64) -> u32 {
    if size <= 1 {
        return 0;
    }
    let max = size - 1;
    ((max as f64) * ratio).round().clamp(0.0, max as f64) as u32
}

fn normalize_screenshot_dimensions(
    data: Vec<u8>,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<u8>> {
    let image = image::load_from_memory(&data).context("Failed to decode QMP screenshot PNG")?;
    if image.width() == target_width && image.height() == target_height {
        return Ok(data);
    }

    let resized = image.resize_exact(target_width, target_height, FilterType::CatmullRom);
    let mut encoded = std::io::Cursor::new(Vec::new());
    resized
        .write_to(&mut encoded, image::ImageFormat::Png)
        .context("Failed to encode normalized screenshot PNG")?;
    Ok(encoded.into_inner())
}

fn qmp_send_events(socket_path: &str, events: Vec<Value>) -> Result<()> {
    qmp_execute(
        socket_path,
        "input-send-event",
        Some(json!({
            "events": events,
        })),
    )
    .map(|_| ())
}

fn qmp_btn_touch(down: bool) -> Value {
    json!({
        "type": "btn",
        "data": {
            "button": "touch",
            "down": down
        }
    })
}

fn qmp_mtt(event_type: &str, slot: i32, tracking_id: i32, axis: &str, value: u32) -> Value {
    json!({
        "type": "mtt",
        "data": {
            "type": event_type,
            "slot": slot,
            "tracking-id": tracking_id,
            "axis": axis,
            "value": value
        }
    })
}

fn qmp_key(qcode: &str, down: bool) -> Value {
    json!({
        "type": "key",
        "data": {
            "down": down,
            "key": {
                "type": "qcode",
                "data": qcode
            }
        }
    })
}

fn qmp_execute(socket_path: &str, command: &str, arguments: Option<Value>) -> Result<Value> {
    let path = Path::new(socket_path);
    if !path.exists() {
        return Err(anyhow!("QMP socket not found: {}", path.display()));
    }

    let mut stream = StdUnixStream::connect(path)?;
    let read_stream = stream.try_clone()?;
    let mut reader = BufReader::new(read_stream);

    let greeting = qmp_read_message(&mut reader)?;
    if greeting.get("QMP").is_none() {
        return Err(anyhow!("Unexpected QMP greeting: {}", greeting));
    }

    qmp_write_message(&mut stream, "qmp_capabilities", None)?;
    let capabilities = qmp_read_message(&mut reader)?;
    if capabilities.get("error").is_some() {
        return Err(anyhow!("QMP qmp_capabilities failed: {}", capabilities));
    }

    qmp_write_message(&mut stream, command, arguments)?;
    let reply = qmp_read_message(&mut reader)?;
    if let Some(error) = reply.get("error") {
        return Err(anyhow!("QMP {} failed: {}", command, error));
    }
    Ok(reply)
}

fn qmp_write_message(
    stream: &mut StdUnixStream,
    command: &str,
    arguments: Option<Value>,
) -> Result<()> {
    let mut payload = json!({ "execute": command });
    if let Some(arguments) = arguments {
        payload["arguments"] = arguments;
    }
    writeln!(stream, "{}", payload)?;
    stream.flush()?;
    Ok(())
}

fn qmp_read_message(reader: &mut BufReader<StdUnixStream>) -> Result<Value> {
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Err(anyhow!("QMP connection closed"));
        }
        let message: Value = serde_json::from_str(line.trim())?;
        if message.get("event").is_some() {
            continue;
        }
        return Ok(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keyboard_height_from_gdbus_output() {
        assert_eq!(
            parse_first_float("(471.6666259765625,)"),
            Some(471.6666259765625)
        );
    }

    #[test]
    fn clipboard_button_center_uses_keyboard_height() {
        let geometry = ResolvedGeometry {
            native_width: 720,
            native_height: 1600,
            visible_width: 720,
            visible_height: 1600,
            orientation: Orientation::Portrait,
            abs_max: 32767,
        };

        assert_eq!(
            geometry.clipboard_button_center(471.6666259765625).unwrap(),
            (47, 1176)
        );
    }
}
