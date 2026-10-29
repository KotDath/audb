//! Kernel panel size for the privileged uinput node.
//!
//! DRM wins when a connector exists. Framebuffer sysfs is consulted only when
//! no DRM connector exists, so KMS devices keep their current geometry.
//! `mode` can exist and be empty; `modes` is the required fallback. Within
//! fbdev the order is the framebuffer var (`FBIOGET_VSCREENINFO`), then a
//! non-empty `mode`, then the first `modes` line. The var is used only when the
//! axes are native (`rotate` 0 or 180) and, when the kernel publishes a `modes`
//! list, when the size is in it; with no list at all the var is kept unchecked.
//! `virtual_size` and `stride` describe a stride-aligned buffer, not the panel.
use crate::Display;
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
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

struct DrmScan {
    connected: Vec<PathBuf>,
    saw_connector: bool,
}

/// What asking the kernel for the framebuffer var produced.
///
/// `Unavailable` and `Unusable` are deliberately distinct: a var that was read
/// but cannot be decoded still produces a diagnostic note instead of a silent
/// fallback with no explanation.
#[derive(Debug, PartialEq, Eq)]
enum FbVar {
    /// Missing device node, denied open, or a rejected ioctl.
    Unavailable,
    /// The visible size the ioctl reported.
    Size(i32, i32),
    /// The ioctl answered, but the value cannot be used.
    Unusable(String),
}

/// Where the kernel panel size came from. Reported by agent status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSource {
    Drm,
    FbdevVar,
    FbdevMode,
    FbdevModes,
}

impl PanelSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Drm => "drm",
            Self::FbdevVar => "fbdev-var",
            Self::FbdevMode => "fbdev-mode",
            Self::FbdevModes => "fbdev-modes",
        }
    }
}

/// The size fixed into the uinput node, and how it was obtained.
#[derive(Debug, Clone)]
pub struct PanelGeometry {
    pub display: Display,
    pub source: PanelSource,
    /// Set when the framebuffer var was readable but deliberately not used.
    pub note: Option<String>,
}

pub fn panel_geometry(sysfs: &Path) -> Result<PanelGeometry, PanelError> {
    panel_geometry_with(sysfs, &fb_var_size)
}

/// `fb_var` is a seam: the agent passes the framebuffer ioctl, while tests
/// pass a fake so a sysfs fixture never reads the host's own framebuffer.
fn panel_geometry_with(
    sysfs: &Path,
    fb_var: &dyn Fn(&Path) -> FbVar,
) -> Result<PanelGeometry, PanelError> {
    let scan = drm_scan(sysfs)?;
    if scan.connected.len() > 1 {
        return Err(PanelError::Ambiguous(format!(
            "Expected one connected DRM panel, found {}",
            scan.connected.len()
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
        return Ok(PanelGeometry {
            display: display_from(width, height)?,
            source: PanelSource::Drm,
            note: None,
        });
    }
    if scan.saw_connector {
        return Err(PanelError::Unavailable(
            "Expected one connected DRM panel, found 0".into(),
        ));
    }
    fbdev_panel(sysfs, fb_var)
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
        // `version` is a file. Connectors are directories. Sysfs publishes
        // them as symlinks, and `DirEntry::metadata` does not follow those.
        let metadata = fs::metadata(entry.path()).map_err(|error| {
            PanelError::ReadFailed(format!("Cannot read {}: {error}", entry.path().display()))
        })?;
        if !metadata.is_dir() {
            continue;
        }
        match read_text(&entry.path().join("status"))? {
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

fn fbdev_panel(sysfs: &Path, fb_var: &dyn Fn(&Path) -> FbVar) -> Result<PanelGeometry, PanelError> {
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
            "Expected one framebuffer, found {}",
            framebuffers.len()
        )));
    }
    let Some(framebuffer) = framebuffers.first() else {
        return Err(PanelError::Unavailable(
            "No DRM connector or framebuffer was found".into(),
        ));
    };
    framebuffer_size(framebuffer, fb_var)
}

fn is_framebuffer(name: &str) -> bool {
    let Some(index) = name.strip_prefix("fb") else {
        return false;
    };
    !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
}

fn framebuffer_size(
    fb: &Path,
    fb_var: &dyn Fn(&Path) -> FbVar,
) -> Result<PanelGeometry, PanelError> {
    let name = fb
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("fb");
    // `modes` is read first because it validates the var. `mode` is lower
    // priority, so it is read only after the var has been rejected: an
    // unreadable `mode` must not block a usable var.
    let modes = read_text(&fb.join("modes"))?;
    let listed = listed_sizes(modes.as_deref());
    let mut note = None;
    match fb_var(fb) {
        FbVar::Size(width, height) => {
            match fb_var_reject_reason(fb, (width, height), listed.as_deref()) {
                None => {
                    return Ok(PanelGeometry {
                        display: display_from(width, height)?,
                        source: PanelSource::FbdevVar,
                        note: None,
                    })
                }
                Some(reason) => {
                    note = Some(format!(
                        "Framebuffer var {width}x{height} was not used: {reason}"
                    ));
                }
            }
        }
        FbVar::Unusable(reason) => {
            note = Some(format!(
                "Framebuffer var was read but is unusable: {reason}"
            ))
        }
        FbVar::Unavailable => {}
    }
    let mode = read_text(&fb.join("mode"))?;
    if let Some(text) = mode.as_deref().filter(|text| !text.trim().is_empty()) {
        let (width, height) = parse_fbdev_mode(text)?;
        return Ok(PanelGeometry {
            display: display_from(width, height)?,
            source: PanelSource::FbdevMode,
            note,
        });
    }
    match modes.as_deref().filter(|text| !text.trim().is_empty()) {
        Some(text) => {
            let (width, height) = parse_fbdev_mode(text)?;
            Ok(PanelGeometry {
                display: display_from(width, height)?,
                source: PanelSource::FbdevModes,
                note,
            })
        }
        None => Err(PanelError::Unavailable(format!(
            "Framebuffer {name} has no mode"
        ))),
    }
}

/// Why the framebuffer var is not a usable panel size, if it is not.
///
/// The var is the current size, but the node needs native axes and a size the
/// kernel itself declares: `mtkfb` swaps `xres`/`yres` in `mtkfb_check_var`
/// when rotate is 90 or 270, and a driver may report a stride-aligned value.
///
/// `listed` is `None` when the kernel publishes no `modes` list at all, which
/// is the only case where the var is accepted without a membership check.
fn fb_var_reject_reason(
    fb: &Path,
    size: (i32, i32),
    listed: Option<&[(i32, i32)]>,
) -> Option<String> {
    if display_from(size.0, size.1).is_err() {
        return Some("it is outside the supported 2..=16384 range".into());
    }
    // `fb_var_screeninfo.rotate` is documented in degrees and drivers use the
    // quarter turns. Only 0 and 180 keep the axis order, so allow exactly
    // those: an unexpected value must not be treated as "not rotated".
    match fb_rotate(fb) {
        Some(0 | 180) => {}
        Some(degrees) => {
            return Some(format!(
                "fb rotate is {degrees}; only 0 and 180 keep the var axes native"
            ))
        }
        None => return Some("the fb rotate value is unreadable".into()),
    }
    match listed {
        None => {}
        Some([]) => return Some("the kernel modes list has no usable entry".into()),
        Some(listed) if !listed.contains(&size) => {
            return Some("it is not one of the modes the kernel lists".into())
        }
        Some(_) => {}
    }
    None
}

fn fb_rotate(fb: &Path) -> Option<i32> {
    fs::read_to_string(fb.join("rotate"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The sizes the kernel lists, or `None` when there is no `modes` text or it
/// is blank.
///
/// An empty vector therefore means the file exists but holds no usable mode,
/// which is deliberately different from no list at all: a wholly malformed
/// list must not silently disable the membership check.
fn listed_sizes(text: Option<&str>) -> Option<Vec<(i32, i32)>> {
    let text = text?;
    if text.trim().is_empty() {
        return None;
    }
    let mut sizes = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Ok(size) = first_wxh(line) {
            if !sizes.contains(&size) {
                sizes.push(size);
            }
        }
    }
    Some(sizes)
}

/// `FBIOGET_VSCREENINFO` from `linux/fb.h`.
///
/// The header defines the classic fbdev requests as plain 16-bit values, with
/// the comment "ioctls 0x46 is 'F'": this is NOT `_IOR('F', 0, ...)` encoded.
/// `0x80A04600` and other derived values are rejected with `ENOTTY`, and
/// `FBIO_CURSOR` is the newer request that does use `_IOWR`.
const FBIOGET_VSCREENINFO: libc::c_ulong = 0x4600;

/// `sizeof(struct fb_var_screeninfo)`. The struct is all `__u32` fields, so the
/// size is the same on 32 and 64 bit kernels, and the kernel writes all of it.
const FB_VAR_SCREENINFO_LEN: usize = 160;

/// The current framebuffer size from `FBIOGET_VSCREENINFO`.
///
/// Best effort: a missing device node, a denied open (the service unit allows
/// only `/dev/uinput` and `/dev/fb0`), or a driver that rejects the ioctl all
/// yield `FbVar::Unavailable`, so the text fallbacks keep working.
fn fb_var_size(fb: &Path) -> FbVar {
    let Some(name) = fb.file_name().and_then(|name| name.to_str()) else {
        return FbVar::Unavailable;
    };
    let Ok(file) = fs::File::open(Path::new("/dev").join(name)) else {
        return FbVar::Unavailable;
    };
    let mut buffer = [0u8; FB_VAR_SCREENINFO_LEN];
    if unsafe { libc::ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO, buffer.as_mut_ptr()) } < 0 {
        return FbVar::Unavailable;
    }
    decode_fb_var(&buffer)
}

/// `xres`/`yres`, the visible resolution, from a `struct fb_var_screeninfo`.
///
/// `xres_virtual`/`yres_virtual` at offsets 8 and 12 are the aligned buffer and
/// are deliberately not read. Fixed-index reads on a fixed-size array cannot
/// panic, so no error path is needed for the slice itself.
fn decode_fb_var(buffer: &[u8; FB_VAR_SCREENINFO_LEN]) -> FbVar {
    let xres = u32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
    let yres = u32::from_ne_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
    let (Ok(width), Ok(height)) = (i32::try_from(xres), i32::try_from(yres)) else {
        return FbVar::Unusable(format!("it reports {xres}x{yres}, not a 32-bit size"));
    };
    if width <= 0 || height <= 0 {
        return FbVar::Unusable(format!("it reports {width}x{height}"));
    }
    FbVar::Size(width, height)
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

/// The first non-empty line of the fbdev `mode`/`modes` text, used when the
/// framebuffer var is unavailable or not trusted.
///
/// This is best-effort selection, not active-mode detection. The kernel core
/// prepends the active mode to `fb_info->modelist` in `do_register_framebuffer`
/// and `fb_set_var`, but `fb_add_videomode` prepends only unmatched entries, so
/// a driver-populated list can keep a non-active mode at its head. A wrong pick
/// costs touch: the agent reports the disagreement instead of guessing another
/// line, and no multi-mode device has been demonstrated.
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

fn display_from(width: i32, height: i32) -> Result<Display, PanelError> {
    let display = Display {
        native_width: width,
        native_height: height,
        rotation: 0,
    };
    display.validate().map_err(|_| {
        PanelError::OutOfRange(format!("Panel size {width}x{height} is outside 2..=16384"))
    })?;
    Ok(display)
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

    /// Tests inject the framebuffer var: a fake keeps a sysfs fixture from
    /// reading the host's own `/dev/fb0`, which only the agent does.
    fn geometry_with_probe(
        sysfs_root: &Path,
        probe: &dyn Fn(&Path) -> FbVar,
    ) -> Result<PanelGeometry, PanelError> {
        panel_geometry_with(sysfs_root, probe)
    }

    fn geometry(sysfs_root: &Path) -> Result<(i32, i32), PanelError> {
        geometry_with_probe(sysfs_root, &|_| FbVar::Unavailable).map(|panel| {
            assert_eq!(
                panel.display.rotation, 0,
                "kernel geometry is not orientation"
            );
            (panel.display.native_width, panel.display.native_height)
        })
    }

    fn size(panel: &PanelGeometry) -> (i32, i32) {
        (panel.display.native_width, panel.display.native_height)
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
    fn multi_line_fbdev_modes_selection_follows_the_list_order() {
        // Documented policy, not active-mode detection: the first line wins,
        // because the kernel prepends only unmatched modes and a driver-built
        // list can keep a non-active mode at its head. Reordering the same
        // sizes changes the result, so both orders are pinned here on purpose.
        let first = sysfs(&[
            ("class/graphics/fb0/mode", ""),
            (
                "class/graphics/fb0/modes",
                "U:1080x2400p-60\nU:720x1600p-60\n",
            ),
        ]);
        let second = sysfs(&[
            ("class/graphics/fb0/mode", ""),
            (
                "class/graphics/fb0/modes",
                "U:720x1600p-60\nU:1080x2400p-60\n",
            ),
        ]);
        assert_eq!(geometry(first.path()).unwrap(), (1080, 2400));
        assert_eq!(geometry(second.path()).unwrap(), (720, 1600));
    }

    #[test]
    fn absent_or_blank_modes_list_does_not_reject_the_var() {
        // No published list is the only case that skips membership.
        for modes in [None, Some(""), Some("\n  \n")] {
            let mut files = vec![
                ("class/graphics/fb0/mode", ""),
                ("class/graphics/fb0/rotate", "0\n"),
            ];
            if let Some(text) = modes {
                files.push(("class/graphics/fb0/modes", text));
            }
            let dir = sysfs(&files);
            let panel = geometry_with_probe(dir.path(), &|_| FbVar::Size(720, 1600)).unwrap();
            assert_eq!(panel.source, PanelSource::FbdevVar, "modes {modes:?}");
            assert!(panel.note.is_none(), "modes {modes:?}");
        }
    }

    #[test]
    fn mixed_valid_and_malformed_modes_lines_still_constrain_the_var() {
        // A usable line establishes membership even when another line is junk.
        let valid_first = sysfs(&[
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", "U:720x1600p-60\nbogus\n"),
            ("class/graphics/fb0/rotate", "0\n"),
        ]);
        let panel = geometry_with_probe(valid_first.path(), &|_| FbVar::Size(720, 1600)).unwrap();
        assert_eq!(panel.source, PanelSource::FbdevVar);
        let panel = geometry_with_probe(valid_first.path(), &|_| FbVar::Size(736, 1600)).unwrap();
        assert_eq!(size(&panel), (720, 1600));
        assert_eq!(panel.source, PanelSource::FbdevModes);
        assert!(panel.note.unwrap().contains("not one of the modes"));

        // With the junk line first the fallback text itself is malformed, but
        // membership still rejects the var before that fallback is reached.
        let junk_first = sysfs(&[
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", "bogus\nU:720x1600p-60\n"),
            ("class/graphics/fb0/rotate", "0\n"),
        ]);
        let error =
            geometry_with_probe(junk_first.path(), &|_| FbVar::Size(736, 1600)).unwrap_err();
        assert_eq!(error.reason_code(), "PANEL_GEOMETRY_INVALID");
    }

    #[test]
    fn framebuffer_var_wins_over_the_first_listed_mode() {
        // The var is the active size, so a multi-mode list no longer decides it.
        let dir = sysfs(&[
            ("class/graphics/fb0/mode", ""),
            (
                "class/graphics/fb0/modes",
                "U:720x1600p-60\nU:1080x2400p-60\n",
            ),
            ("class/graphics/fb0/rotate", "0\n"),
        ]);
        let panel = geometry_with_probe(dir.path(), &|_| FbVar::Size(1080, 2400)).unwrap();
        assert_eq!(size(&panel), (1080, 2400));
        assert_eq!(panel.source, PanelSource::FbdevVar);
        assert!(panel.note.is_none());
    }

    #[test]
    fn framebuffer_var_outside_the_listed_modes_falls_back_with_a_note() {
        // A stride-aligned 736x1600 must not become the panel.
        let dir = sysfs(&[
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", "U:720x1600p-60\n"),
            ("class/graphics/fb0/rotate", "0\n"),
        ]);
        let panel = geometry_with_probe(dir.path(), &|_| FbVar::Size(736, 1600)).unwrap();
        assert_eq!(size(&panel), (720, 1600));
        assert_eq!(panel.source, PanelSource::FbdevModes);
        let note = panel.note.unwrap();
        assert!(note.contains("736x1600"), "{note}");
        assert!(note.contains("not one of the modes"), "{note}");
    }

    #[test]
    fn framebuffer_var_rotation_allowlist() {
        // Only 0 and 180 keep the var axes native. 90 and 270 swap them in
        // mtkfb's mtkfb_check_var, and any other integer is not a documented
        // quarter turn, so it must not be accepted as "not rotated".
        for degrees in [0, 180] {
            let rotate = format!("{degrees}\n");
            let dir = sysfs(&[
                ("class/graphics/fb0/mode", ""),
                ("class/graphics/fb0/modes", "U:720x1600p-60\n"),
                ("class/graphics/fb0/rotate", rotate.as_str()),
            ]);
            let panel = geometry_with_probe(dir.path(), &|_| FbVar::Size(720, 1600)).unwrap();
            assert_eq!(panel.source, PanelSource::FbdevVar, "rotate {degrees}");
        }
        for degrees in [90, 270, 1, 3, -90, 999] {
            let rotate = format!("{degrees}\n");
            let dir = sysfs(&[
                ("class/graphics/fb0/mode", ""),
                ("class/graphics/fb0/modes", "U:720x1600p-60\n"),
                ("class/graphics/fb0/rotate", rotate.as_str()),
            ]);
            let panel = geometry_with_probe(dir.path(), &|_| FbVar::Size(720, 1600)).unwrap();
            assert_eq!(panel.source, PanelSource::FbdevModes, "rotate {degrees}");
            let note = panel.note.unwrap();
            assert!(note.contains(&format!("rotate is {degrees}")), "{note}");
        }
    }

    #[test]
    fn malformed_modes_do_not_disable_membership() {
        // A non-empty but unusable list must not be treated as "no list".
        let dir = sysfs(&[
            ("class/graphics/fb0/mode", "U:720x1600p-60\n"),
            ("class/graphics/fb0/modes", "bogus\n"),
            ("class/graphics/fb0/rotate", "0\n"),
        ]);
        let panel = geometry_with_probe(dir.path(), &|_| FbVar::Size(736, 1600)).unwrap();
        assert_eq!(size(&panel), (720, 1600));
        assert_eq!(panel.source, PanelSource::FbdevMode);
        let note = panel.note.unwrap();
        assert!(note.contains("736x1600"), "{note}");
        assert!(note.contains("no usable entry"), "{note}");
    }

    #[test]
    fn framebuffer_var_is_used_when_mode_is_unreadable() {
        // `mode` is lower priority than the var, so its read error must not
        // block an otherwise acceptable var.
        let dir = sysfs(&[
            ("class/graphics/fb0/modes", "U:720x1600p-60\n"),
            ("class/graphics/fb0/rotate", "0\n"),
        ]);
        fs::create_dir(dir.path().join("class/graphics/fb0/mode")).unwrap();
        let panel = geometry_with_probe(dir.path(), &|_| FbVar::Size(720, 1600)).unwrap();
        assert_eq!(size(&panel), (720, 1600));
        assert_eq!(panel.source, PanelSource::FbdevVar);
    }

    #[test]
    fn fb_get_vscreeninfo_uses_the_legacy_fb_encoding() {
        // Guards against "fixing" this into an _IOR-derived value such as
        // 0x80A04600, which the kernel answers with ENOTTY. linux/fb.h:
        // "ioctls 0x46 is 'F'", then #define FBIOGET_VSCREENINFO 0x4600;
        // FBIO_CURSOR is the request that uses _IOWR.
        assert_eq!(FBIOGET_VSCREENINFO, 0x4600);
    }

    #[test]
    fn decode_fb_var_reads_the_visible_resolution() {
        let mut buffer = [0u8; FB_VAR_SCREENINFO_LEN];
        buffer[0..4].copy_from_slice(&720u32.to_ne_bytes());
        buffer[4..8].copy_from_slice(&1600u32.to_ne_bytes());
        buffer[8..12].copy_from_slice(&736u32.to_ne_bytes());
        buffer[12..16].copy_from_slice(&4800u32.to_ne_bytes());
        assert_eq!(decode_fb_var(&buffer), FbVar::Size(720, 1600));
        // Unusable values carry a reason instead of vanishing into a fallback.
        assert_eq!(
            decode_fb_var(&[0u8; FB_VAR_SCREENINFO_LEN]),
            FbVar::Unusable("it reports 0x0".into())
        );
        let mut overflow = [0u8; FB_VAR_SCREENINFO_LEN];
        overflow[0..4].copy_from_slice(&u32::MAX.to_ne_bytes());
        overflow[4..8].copy_from_slice(&1600u32.to_ne_bytes());
        assert_eq!(
            decode_fb_var(&overflow),
            FbVar::Unusable("it reports 4294967295x1600, not a 32-bit size".into())
        );
    }

    #[test]
    fn unusable_framebuffer_var_still_produces_a_note() {
        // Read but undecodable: the fallback must still explain itself.
        let dir = sysfs(&[
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", "U:720x1600p-60\n"),
            ("class/graphics/fb0/rotate", "0\n"),
        ]);
        let probe = &|_: &Path| FbVar::Unusable("it reports 0x0".into());
        let panel = geometry_with_probe(dir.path(), probe).unwrap();
        assert_eq!(size(&panel), (720, 1600));
        assert_eq!(panel.source, PanelSource::FbdevModes);
        let note = panel.note.unwrap();
        assert!(note.contains("unusable"), "{note}");
        assert!(note.contains("0x0"), "{note}");
    }

    #[test]
    fn implausible_framebuffer_var_falls_back() {
        let dir = sysfs(&[
            ("class/graphics/fb0/mode", "U:1080x2400p-0\n"),
            ("class/graphics/fb0/modes", "U:1080x2400p-0\n"),
            ("class/graphics/fb0/rotate", "0\n"),
        ]);
        for bad in [(0, 0), (1, 1600), (20000, 1600)] {
            let panel = geometry_with_probe(dir.path(), &|_| FbVar::Size(bad.0, bad.1)).unwrap();
            assert_eq!(panel.source, PanelSource::FbdevMode, "{bad:?}");
            assert_eq!(size(&panel), (1080, 2400), "{bad:?}");
            assert!(panel.note.is_some(), "{bad:?}");
        }
    }

    #[test]
    fn framebuffer_var_needs_a_readable_rotate_value() {
        let dir = sysfs(&[
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", "U:720x1600p-60\n"),
        ]);
        let panel = geometry_with_probe(dir.path(), &|_| FbVar::Size(720, 1600)).unwrap();
        assert_eq!(panel.source, PanelSource::FbdevModes);
        assert!(panel.note.unwrap().contains("rotate"));
    }

    #[test]
    fn framebuffer_var_is_used_when_the_kernel_lists_no_modes() {
        let dir = sysfs(&[
            ("class/graphics/fb0/mode", ""),
            ("class/graphics/fb0/modes", ""),
            ("class/graphics/fb0/rotate", "0\n"),
        ]);
        let panel = geometry_with_probe(dir.path(), &|_| FbVar::Size(720, 1600)).unwrap();
        assert_eq!(size(&panel), (720, 1600));
        assert_eq!(panel.source, PanelSource::FbdevVar);
    }

    #[test]
    fn drm_panel_does_not_probe_the_framebuffer_var() {
        let dir = sysfs(&[
            ("class/drm/card0-DSI-1/status", "connected\n"),
            ("class/drm/card0-DSI-1/modes", "1080x2340\n"),
        ]);
        let panel =
            geometry_with_probe(dir.path(), &|_| panic!("DRM must not read the framebuffer"))
                .unwrap();
        assert_eq!(size(&panel), (1080, 2340));
        assert_eq!(panel.source, PanelSource::Drm);
        assert!(panel.note.is_none());
    }

    #[test]
    fn framebuffer_var_does_not_hide_an_ambiguous_framebuffer() {
        let dir = sysfs(&[
            ("class/graphics/fb0/modes", "U:720x1600p-60\n"),
            ("class/graphics/fb1/modes", "U:1080x2400p-60\n"),
        ]);
        let error = geometry_with_probe(dir.path(), &|_| panic!("no single framebuffer to probe"))
            .unwrap_err();
        assert_eq!(error.reason_code(), "PANEL_GEOMETRY_AMBIGUOUS");
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
