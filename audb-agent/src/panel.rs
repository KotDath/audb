//! Kernel panel size for the privileged uinput node.
//!
//! DRM wins when a connector exists. Framebuffer sysfs is consulted only when
//! no DRM connector exists, so KMS devices keep their current geometry.
//! `mode` can exist and be empty; `modes` is the required fallback.
//! `virtual_size` and `stride` describe a stride-aligned buffer, not the panel.
use crate::Display;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where the kernel-native panel size was read from. Reported so a device can be
/// diagnosed without inferring the source from which probe happened to succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSource {
    Drm,
    Fbdev,
}

impl PanelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Drm => "drm",
            Self::Fbdev => "fbdev",
        }
    }
}

/// The kernel panel together with the node it was read from.
#[derive(Debug)]
pub struct PanelGeometry {
    pub display: Display,
    pub source: PanelSource,
    pub node: String,
}

#[derive(Debug)]
pub enum PanelError {
    Unavailable(String),
    Ambiguous(String),
    Invalid(String),
    OutOfRange(String),
    ReadFailed(String),
}

impl PanelError {
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "PANEL_GEOMETRY_UNAVAILABLE",
            Self::Ambiguous(_) => "PANEL_GEOMETRY_AMBIGUOUS",
            Self::Invalid(_) => "PANEL_GEOMETRY_INVALID",
            Self::OutOfRange(_) => "PANEL_GEOMETRY_OUT_OF_RANGE",
            Self::ReadFailed(_) => "PANEL_GEOMETRY_READ_FAILED",
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Unavailable(message)
            | Self::Ambiguous(message)
            | Self::Invalid(message)
            | Self::OutOfRange(message)
            | Self::ReadFailed(message) => message,
        }
    }
}

impl std::fmt::Display for PanelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for PanelError {}
struct DrmScan {
    connected: Vec<PathBuf>,
    saw_connector: bool,
}

pub fn panel_geometry(sysfs: &Path) -> Result<PanelGeometry, PanelError> {
    let scan = drm_scan(sysfs)?;
    if scan.connected.len() > 1 {
        return Err(PanelError::Ambiguous(format!(
            "Expected one connected DRM panel, found {}: {}",
            scan.connected.len(),
            node_names(&scan.connected)
        )));
    }
    if let Some(connector) = scan.connected.first() {
        let modes = read_text(&connector.join("modes"))?.ok_or_else(|| {
            PanelError::ReadFailed(format!(
                "DRM connector {} is missing its modes file",
                connector.display()
            ))
        })?;
        let (width, height) = parse_drm_mode(&modes)?;
        return geometry_from(width, height, PanelSource::Drm, connector);
    }
    if scan.saw_connector {
        return Err(PanelError::Unavailable(
            "Expected one connected DRM panel, found 0".into(),
        ));
    }
    fbdev_panel(sysfs)
}

fn node_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("panel")
        .to_owned()
}

fn node_names(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| node_name(path))
        .collect::<Vec<_>>()
        .join(", ")
}

fn drm_scan(sysfs: &Path) -> Result<DrmScan, PanelError> {
    let dir = sysfs.join("class/drm");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(DrmScan {
                connected: Vec::new(),
                saw_connector: false,
            });
        }
        Err(error) => {
            return Err(PanelError::ReadFailed(format!(
                "Cannot read {}: {error}",
                dir.display()
            )));
        }
    };
    let mut scan = DrmScan {
        connected: Vec::new(),
        saw_connector: false,
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            PanelError::ReadFailed(format!("Cannot read {}: {error}", dir.display()))
        })?;
        // A dangling connector symlink is a leftover rather than a panel, so skip that entry
        // instead of failing the whole scan.
        let metadata = match fs::metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(PanelError::ReadFailed(format!(
                    "Cannot read {}: {error}",
                    entry.path().display()
                )))
            }
        };
        if !metadata.is_dir() {
            continue;
        }
        // `version` is a file and `cardN` is the card node; connectors are
        // `card<N>-<connector>-<type>-<id>`. A connector must be honoured even when its status
        // file cannot be read, otherwise an unreadable connector would silently fall back to
        // fbdev on a device that does have DRM.
        let connector = is_connector(&entry.file_name().to_string_lossy());
        if connector {
            scan.saw_connector = true;
        }
        match read_text(&entry.path().join("status"))? {
            None if connector => {
                return Err(PanelError::ReadFailed(format!(
                    "DRM connector {} is missing its status file",
                    entry.path().display()
                )))
            }
            None => {}
            Some(status) => {
                scan.saw_connector = true;
                if status.trim() == "connected" {
                    scan.connected.push(entry.path());
                }
            }
        }
    }
    Ok(scan)
}

/// DRM connectors are published as `card<N>-<connector>-<type>-<id>` (for example
/// `card0-DSI-1`). `card0` itself and `version` are not connectors.
fn is_connector(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("card") else {
        return false;
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && rest.as_bytes().get(digits) == Some(&b'-')
}

fn fbdev_panel(sysfs: &Path) -> Result<PanelGeometry, PanelError> {
    let dir = sysfs.join("class/graphics");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(PanelError::Unavailable(
                "No DRM connector or framebuffer was found".into(),
            ));
        }
        Err(error) => {
            return Err(PanelError::ReadFailed(format!(
                "Cannot read {}: {error}",
                dir.display()
            )));
        }
    };
    let mut framebuffers = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            PanelError::ReadFailed(format!("Cannot read {}: {error}", dir.display()))
        })?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if is_framebuffer(&name) {
            framebuffers.push(entry.path());
        }
    }
    if framebuffers.len() > 1 {
        return Err(PanelError::Ambiguous(format!(
            "Expected one framebuffer, found {}: {}",
            framebuffers.len(),
            node_names(&framebuffers)
        )));
    }
    let Some(framebuffer) = framebuffers.first() else {
        return Err(PanelError::Unavailable(
            "No DRM connector or framebuffer was found".into(),
        ));
    };
    framebuffer_size(framebuffer)
}

fn is_framebuffer(name: &str) -> bool {
    let Some(index) = name.strip_prefix("fb") else {
        return false;
    };
    !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
}

fn framebuffer_size(fb: &Path) -> Result<PanelGeometry, PanelError> {
    let mode = read_text(&fb.join("mode"))?;
    let text = match mode {
        Some(text) if !text.trim().is_empty() => text,
        _ => match read_text(&fb.join("modes"))? {
            Some(text) if !text.trim().is_empty() => text,
            _ => {
                return Err(PanelError::Unavailable(format!(
                    "Framebuffer {} has no mode",
                    fb.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("fb")
                )))
            }
        },
    };
    let (width, height) = parse_fbdev_mode(&text)?;
    geometry_from(width, height, PanelSource::Fbdev, fb)
}

fn read_text(path: &Path) -> Result<Option<String>, PanelError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(PanelError::ReadFailed(format!(
            "Cannot read {}: {error}",
            path.display()
        ))),
    }
}

fn parse_drm_mode(text: &str) -> Result<(i32, i32), PanelError> {
    // Same rule as before: the first modes line is WxH, and a non-numeric
    // token fails the connector. Do not scan this text for a later WxH.
    let Some(line) = text.lines().next() else {
        return Err(PanelError::Invalid("DRM connector mode is empty".into()));
    };
    if line.is_empty() {
        return Err(PanelError::Invalid("DRM connector mode is empty".into()));
    }
    let Some((width, height)) = line.split_once('x') else {
        return Err(PanelError::Invalid(format!(
            "DRM connector mode is malformed: {line}"
        )));
    };
    Ok((parse_drm_i32(width, line)?, parse_drm_i32(height, line)?))
}

fn parse_drm_i32(token: &str, line: &str) -> Result<i32, PanelError> {
    token
        .parse::<i32>()
        .map_err(|_| PanelError::Invalid(format!("DRM connector mode is malformed: {line}")))
}

fn parse_fbdev_mode(text: &str) -> Result<(i32, i32), PanelError> {
    let Some(line) = text.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return Err(PanelError::Invalid("fbdev mode is empty".into()));
    };
    first_wxh(line)
}

fn first_wxh(line: &str) -> Result<(i32, i32), PanelError> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let width_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let width_end = index;
        if index >= bytes.len() || bytes[index] != b'x' {
            continue;
        }
        index += 1;
        if index >= bytes.len() || !bytes[index].is_ascii_digit() {
            continue;
        }
        let height_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let width = &line[width_start..width_end];
        let height = &line[height_start..index];
        return Ok((parse_fb_i32(width)?, parse_fb_i32(height)?));
    }
    Err(PanelError::Invalid(format!(
        "fbdev mode is malformed: {line}"
    )))
}

fn parse_fb_i32(token: &str) -> Result<i32, PanelError> {
    token.parse::<i32>().map_err(|_| {
        PanelError::Invalid(format!(
            "fbdev mode number is not a 32-bit integer: {token}"
        ))
    })
}

fn geometry_from(
    width: i32,
    height: i32,
    source: PanelSource,
    node: &Path,
) -> Result<PanelGeometry, PanelError> {
    let display = Display {
        native_width: width,
        native_height: height,
        rotation: 0,
    };
    display.validate().map_err(|_| {
        PanelError::OutOfRange(format!("Panel size {width}x{height} is outside 2..=16384"))
    })?;
    Ok(PanelGeometry {
        display,
        source,
        node: node_name(node),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Measured on mtkfb: 14 bytes, including the trailing newline.
    const FB_MODES: &str = "U:720x1600p-0\n";

    fn sysfs(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, content) in files {
            let path = dir.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
        }
        dir
    }

    fn geometry(sysfs_root: &Path) -> Result<(i32, i32), PanelError> {
        panel_geometry(sysfs_root).map(|panel| {
            assert_eq!(
                panel.display.rotation, 0,
                "kernel geometry is not orientation"
            );
            (panel.display.native_width, panel.display.native_height)
        })
    }

    /// Measured on the Fplus T1100 (mtkfb): there is no DRM class at all, `mode` is
    /// empty, `modes` carries the panel, and `virtual_size` is a triple buffer
    /// (2176 x 3) that must never be mistaken for the panel.
    #[test]
    fn t1100_mtkfb_uses_modes_and_rejects_virtual_size() {
        let dir = sysfs(&[
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", "U:1600x2176p-0\n"),
            ("class/graphics/fb0/virtual_size", "1600,6528\n"),
            ("class/graphics/fb0/stride", "6400\n"),
            ("class/graphics/fb0/bits_per_pixel", "32\n"),
            ("class/graphics/fb0/name", "mtkfb\n"),
        ]);
        assert!(!dir.path().join("class/drm").exists());
        let panel = panel_geometry(dir.path()).unwrap();
        assert_eq!(
            (panel.display.native_width, panel.display.native_height),
            (1600, 2176)
        );
        assert_eq!(panel.source, PanelSource::Fbdev);
        assert_eq!(panel.node, "fb0");
        assert_ne!(
            panel.display.native_height, 6528,
            "virtual_size is a buffer, not the panel"
        );
    }

    /// A connector whose status file cannot be read must not silently fall back to fbdev.
    #[test]
    fn unreadable_connector_status_does_not_fall_back_to_fbdev() {
        let dir = sysfs(&[
            ("class/drm/card0-DSI-1/modes", "1080x2340\n"),
            ("class/graphics/fb0/modes", "U:720x1600p-0\n"),
        ]);
        let error = panel_geometry(dir.path()).unwrap_err();
        assert_eq!(error.reason_code(), "PANEL_GEOMETRY_READ_FAILED");
    }

    /// A dangling connector symlink is a leftover, not a panel: fbdev stays usable.
    #[test]
    fn dangling_connector_symlink_still_uses_fbdev() {
        let dir = sysfs(&[
            ("class/drm/version", "drm 1.1.0 20060810\n"),
            ("class/graphics/fb0/modes", "U:720x1600p-0\n"),
        ]);
        std::os::unix::fs::symlink(
            dir.path().join("devices/missing"),
            dir.path().join("class/drm/card0-DSI-1"),
        )
        .unwrap();
        assert_eq!(geometry(dir.path()).unwrap(), (720, 1600));
    }

    /// The card node and the `version` file are not connectors, so fbdev is still selected.
    #[test]
    fn card_node_and_version_are_not_connectors() {
        let dir = sysfs(&[
            ("class/drm/version", "drm 1.1.0 20060810\n"),
            ("class/drm/card0/uevent", "DEVTYPE=drm_minor\n"),
            ("class/graphics/fb0/modes", "U:720x1600p-0\n"),
        ]);
        assert_eq!(geometry(dir.path()).unwrap(), (720, 1600));
    }

    /// Ambiguous framebuffers must name the nodes, not only the count.
    #[test]
    fn multiple_framebuffers_name_the_nodes() {
        let dir = sysfs(&[
            ("class/graphics/fb0/modes", "U:1600x2176p-0\n"),
            ("class/graphics/fb1/modes", "U:1280x720p-0\n"),
        ]);
        let error = panel_geometry(dir.path()).unwrap_err();
        assert_eq!(error.reason_code(), "PANEL_GEOMETRY_AMBIGUOUS");
        let message = error.to_string();
        assert!(
            message.contains("fb0") && message.contains("fb1"),
            "{message}"
        );
    }

    #[test]
    fn single_connected_drm_panel_ignores_fbdev_and_uses_first_mode_line() {
        let dir = sysfs(&[
            ("class/drm/version", "drm 1.1.0 20060810\n"),
            ("class/drm/card0/uevent", "DEVTYPE=drm_minor\n"),
            ("class/drm/card0-HDMI-A-1/status", "disconnected\n"),
            ("class/drm/card0-HDMI-A-1/modes", "1920x1080\n"),
            ("class/drm/card0-DSI-1/status", "connected\n"),
            ("class/drm/card0-DSI-1/modes", "1080x2340\n720x1600\n"),
            ("class/graphics/fb0/mode", FB_MODES),
            ("class/graphics/fb0/virtual_size", "736,4800\n"),
            ("class/graphics/fb0/stride", "2944\n"),
        ]);
        // An unreadable extra framebuffer must not matter when DRM is selected.
        fs::create_dir(dir.path().join("class/graphics/fb1")).unwrap();
        assert_eq!(geometry(dir.path()).unwrap(), (1080, 2340));
    }

    #[test]
    fn drm_connector_symlink_overrides_fbdev() {
        let dir = sysfs(&[
            ("class/drm/version", "drm 1.1.0 20060810\n"),
            ("devices/dsi/status", "connected\n"),
            ("devices/dsi/modes", "1080x2340\n720x1600\n"),
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", FB_MODES),
        ]);
        std::os::unix::fs::symlink(
            dir.path().join("devices/dsi"),
            dir.path().join("class/drm/card0-DSI-1"),
        )
        .unwrap();
        assert_eq!(geometry(dir.path()).unwrap(), (1080, 2340));
    }

    #[test]
    fn version_only_drm_uses_empty_mode_fallback_and_ignores_virtual_size() {
        assert_eq!(FB_MODES.len(), 14);
        let dir = sysfs(&[
            ("class/drm/version", "drm 1.1.0 20060810\n"),
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", FB_MODES),
            ("class/graphics/fb0/virtual_size", "736,4800\n"),
            ("class/graphics/fb0/stride", "2944\n"),
            ("class/graphics/fb0/rotate", "90\n"),
        ]);
        assert_eq!(geometry(dir.path()).unwrap(), (720, 1600));
    }

    #[test]
    fn missing_drm_directory_and_missing_mode_file_use_modes() {
        let dir = sysfs(&[("class/graphics/fb0/modes", FB_MODES)]);
        assert_eq!(geometry(dir.path()).unwrap(), (720, 1600));
    }

    #[test]
    fn whitespace_mode_falls_back_to_modes() {
        let dir = sysfs(&[
            ("class/graphics/fb0/mode", "\n"),
            ("class/graphics/fb0/modes", FB_MODES),
        ]);
        assert_eq!(geometry(dir.path()).unwrap(), (720, 1600));
    }

    #[test]
    fn nonempty_fbdev_mode_wins_over_modes() {
        let dir = sysfs(&[
            ("class/graphics/fb0/mode", "U:1080x2400p-0\n"),
            ("class/graphics/fb0/modes", FB_MODES),
            ("class/graphics/fb0/virtual_size", "736,4800\n"),
        ]);
        assert_eq!(geometry(dir.path()).unwrap(), (1080, 2400));
    }

    #[test]
    fn fbcon_is_not_a_second_framebuffer() {
        let dir = sysfs(&[
            ("class/graphics/fbcon/mode", "bogus\n"),
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", FB_MODES),
        ]);
        assert_eq!(geometry(dir.path()).unwrap(), (720, 1600));
    }

    #[test]
    fn disconnected_drm_connector_does_not_fall_back_to_fbdev() {
        let dir = sysfs(&[
            ("class/drm/card0-DSI-1/status", "disconnected\n"),
            ("class/drm/card0-DSI-1/modes", "1080x2340\n"),
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", FB_MODES),
        ]);
        let error = geometry(dir.path()).unwrap_err();
        assert_eq!(error.reason_code(), "PANEL_GEOMETRY_UNAVAILABLE");
        assert!(error.to_string().contains("found 0"), "{error}");
    }

    #[test]
    fn two_connected_drm_panels_are_ambiguous_even_with_one_fbdev() {
        let dir = sysfs(&[
            ("class/drm/card0-DSI-1/status", "connected\n"),
            ("class/drm/card0-DSI-1/modes", "1080x2340\n"),
            ("class/drm/card0-HDMI-A-1/status", "connected\n"),
            ("class/drm/card0-HDMI-A-1/modes", "1920x1080\n"),
            ("class/graphics/fb0/modes", FB_MODES),
        ]);
        let error = geometry(dir.path()).unwrap_err();
        assert_eq!(error.reason_code(), "PANEL_GEOMETRY_AMBIGUOUS");
        assert!(error.to_string().contains("found 2"), "{error}");
    }

    #[test]
    fn two_framebuffers_are_ambiguous() {
        let dir = sysfs(&[
            ("class/graphics/fb0/modes", FB_MODES),
            ("class/graphics/fb1/modes", "U:1080x2400p-0\n"),
        ]);
        let error = geometry(dir.path()).unwrap_err();
        assert_eq!(error.reason_code(), "PANEL_GEOMETRY_AMBIGUOUS");
    }

    #[test]
    fn malformed_nonempty_mode_does_not_fall_back_to_modes() {
        let dir = sysfs(&[
            ("class/graphics/fb0/mode", "bogus\n"),
            ("class/graphics/fb0/modes", FB_MODES),
        ]);
        let error = geometry(dir.path()).unwrap_err();
        assert_eq!(error.reason_code(), "PANEL_GEOMETRY_INVALID");
    }

    #[test]
    fn malformed_modes_and_drm_mode_stay_invalid() {
        let fb = sysfs(&[
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", "not-a-mode\n"),
        ]);
        assert_eq!(
            geometry(fb.path()).unwrap_err().reason_code(),
            "PANEL_GEOMETRY_INVALID"
        );
        let drm = sysfs(&[
            ("class/drm/card0-DSI-1/status", "connected\n"),
            ("class/drm/card0-DSI-1/modes", "not-a-mode\n"),
            ("class/graphics/fb0/modes", FB_MODES),
        ]);
        assert_eq!(
            geometry(drm.path()).unwrap_err().reason_code(),
            "PANEL_GEOMETRY_INVALID"
        );
    }

    #[test]
    fn out_of_range_is_distinct_from_malformed_and_overflow() {
        let low = sysfs(&[("class/graphics/fb0/modes", "U:1x1600p-0\n")]);
        assert_eq!(
            geometry(low.path()).unwrap_err().reason_code(),
            "PANEL_GEOMETRY_OUT_OF_RANGE"
        );
        let high = sysfs(&[("class/graphics/fb0/modes", "U:20000x1600p-0\n")]);
        assert_eq!(
            geometry(high.path()).unwrap_err().reason_code(),
            "PANEL_GEOMETRY_OUT_OF_RANGE"
        );
        let overflow = sysfs(&[("class/graphics/fb0/modes", "U:9999999999x1600p-0\n")]);
        assert_eq!(
            geometry(overflow.path()).unwrap_err().reason_code(),
            "PANEL_GEOMETRY_INVALID"
        );
    }

    #[test]
    fn unreadable_mode_is_read_failed_without_using_modes() {
        let dir = sysfs(&[
            ("class/graphics/fb0/modes", FB_MODES),
            ("class/graphics/fb0/virtual_size", "736,4800\n"),
        ]);
        fs::create_dir(dir.path().join("class/graphics/fb0/mode")).unwrap();
        assert_eq!(
            geometry(dir.path()).unwrap_err().reason_code(),
            "PANEL_GEOMETRY_READ_FAILED"
        );
    }

    #[test]
    fn no_panel_source_is_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let error = geometry(dir.path()).unwrap_err();
        assert_eq!(error.reason_code(), "PANEL_GEOMETRY_UNAVAILABLE");
        assert!(
            error
                .to_string()
                .contains("No DRM connector or framebuffer"),
            "{error}"
        );
    }

    #[test]
    fn empty_framebuffer_mode_list_is_unavailable() {
        let dir = sysfs(&[
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", ""),
        ]);
        assert_eq!(
            geometry(dir.path()).unwrap_err().reason_code(),
            "PANEL_GEOMETRY_UNAVAILABLE"
        );
    }

    #[test]
    fn drm_modes_read_failure_does_not_use_fbdev() {
        let dir = sysfs(&[
            ("class/drm/card0-DSI-1/status", "connected\n"),
            ("class/graphics/fb0/modes", FB_MODES),
        ]);
        fs::create_dir(dir.path().join("class/drm/card0-DSI-1/modes")).unwrap();
        assert_eq!(
            geometry(dir.path()).unwrap_err().reason_code(),
            "PANEL_GEOMETRY_READ_FAILED"
        );
    }
}
