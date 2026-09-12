use crate::error::{CoreError, CoreResult};
use crate::qmp::QmpClient;
use crate::transport::EmulatorTransport;
use serde_json::json;
use std::path::Path;

#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt;
#[cfg(target_os = "linux")]
use x11rb::connection::Connection;
#[cfg(target_os = "linux")]
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, MapState, Window,
};
#[cfg(target_os = "linux")]
use x11rb::rust_connection::RustConnection;

fn valid_png(data: &[u8]) -> bool {
    data.len() > 100 && data.starts_with(b"\x89PNG\r\n\x1a\n")
}

async fn screendump(qmp: &mut QmpClient) -> CoreResult<Vec<u8>> {
    let path = std::env::temp_dir().join(format!("audb-qmp-shot-{}.png", std::process::id()));
    let _ = tokio::fs::remove_file(&path).await;
    qmp.execute(
        "screendump",
        Some(json!({"filename": path, "format": "png"})),
    )
    .await?;
    let data = tokio::fs::read(&path)
        .await
        .map_err(|e| CoreError::runtime(format!("Cannot read QMP screenshot: {e}")))?;
    let _ = tokio::fs::remove_file(path).await;
    if !valid_png(&data) {
        return Err(CoreError::runtime("QMP returned an invalid PNG"));
    }
    Ok(data)
}

#[cfg(target_os = "linux")]
fn qemu_pid(qmp_socket: &Path) -> CoreResult<u32> {
    let socket = qmp_socket.to_string_lossy();
    let mut matches = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(cmdline) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args = cmdline
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .collect::<Vec<_>>();
        let Some(executable) = args.first() else {
            continue;
        };
        let executable = Path::new(std::ffi::OsStr::from_bytes(executable))
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if !executable.starts_with("qemu-system-") {
            continue;
        }
        if args.iter().any(|arg| {
            std::str::from_utf8(arg).is_ok_and(|arg| qmp_argument_matches(arg, socket.as_ref()))
        }) {
            matches.push(pid);
        }
    }
    match matches.as_slice() {
        [pid] => Ok(*pid),
        [] => Err(CoreError::runtime(format!(
            "Cannot find QEMU process for {}",
            qmp_socket.display()
        ))),
        _ => Err(CoreError::runtime(format!(
            "More than one QEMU process uses {}: {matches:?}",
            qmp_socket.display()
        ))),
    }
}

#[cfg(target_os = "linux")]
fn qmp_argument_matches(argument: &str, socket: &str) -> bool {
    argument
        .strip_prefix("unix:")
        .and_then(|value| value.split(',').next())
        == Some(socket)
}

#[cfg(target_os = "linux")]
fn atom(connection: &RustConnection, name: &[u8]) -> CoreResult<Atom> {
    connection
        .intern_atom(false, name)
        .map_err(|error| CoreError::runtime(format!("X11 intern_atom failed: {error}")))?
        .reply()
        .map(|reply| reply.atom)
        .map_err(|error| CoreError::runtime(format!("X11 intern_atom reply failed: {error}")))
}

#[cfg(target_os = "linux")]
fn property_u32(
    connection: &RustConnection,
    window: Window,
    property: Atom,
    property_type: Atom,
) -> CoreResult<Vec<u32>> {
    let reply = connection
        .get_property(false, window, property, property_type, 0, u32::MAX)
        .map_err(|error| CoreError::runtime(format!("X11 get_property failed: {error}")))?
        .reply()
        .map_err(|error| CoreError::runtime(format!("X11 get_property reply failed: {error}")))?;
    Ok(reply
        .value32()
        .map(|values| values.collect())
        .unwrap_or_default())
}

#[cfg(target_os = "linux")]
fn qemu_window(connection: &RustConnection, screen_num: usize, pid: u32) -> CoreResult<Window> {
    let root = connection.setup().roots[screen_num].root;
    let clients = atom(connection, b"_NET_CLIENT_LIST")?;
    let window_pid = atom(connection, b"_NET_WM_PID")?;
    let windows = property_u32(connection, root, clients, AtomEnum::WINDOW.into())?;
    let mut matches = Vec::new();
    for window in windows {
        let pids = property_u32(connection, window, window_pid, AtomEnum::CARDINAL.into())?;
        if pids.first() != Some(&pid) {
            continue;
        }
        let attributes = connection
            .get_window_attributes(window)
            .map_err(|error| CoreError::runtime(format!("X11 window lookup failed: {error}")))?
            .reply()
            .map_err(|error| {
                CoreError::runtime(format!("X11 window lookup reply failed: {error}"))
            })?;
        if attributes.map_state == MapState::VIEWABLE {
            matches.push(window);
        }
    }
    match matches.as_slice() {
        [window] => Ok(*window),
        [] => Err(CoreError::runtime(format!(
            "Cannot find a visible X11 window for QEMU PID {pid}"
        ))),
        _ => Err(CoreError::runtime(format!(
            "More than one visible X11 window belongs to QEMU PID {pid}: {matches:?}"
        ))),
    }
}

#[cfg(target_os = "linux")]
fn component(pixel: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let maximum = mask >> shift;
    let value = (pixel & mask) >> shift;
    (((u64::from(value) * 255) + u64::from(maximum) / 2) / u64::from(maximum)) as u8
}

#[cfg(target_os = "linux")]
fn host_window(qmp_socket: &Path) -> CoreResult<Vec<u8>> {
    let pid = qemu_pid(qmp_socket)?;
    let (connection, screen_num) = x11rb::connect(None)
        .map_err(|error| CoreError::runtime(format!("Cannot connect to X11: {error}")))?;
    let window = qemu_window(&connection, screen_num, pid)?;
    let geometry = connection
        .get_geometry(window)
        .map_err(|error| CoreError::runtime(format!("X11 get_geometry failed: {error}")))?
        .reply()
        .map_err(|error| CoreError::runtime(format!("X11 get_geometry reply failed: {error}")))?;
    let image = connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            window,
            0,
            0,
            geometry.width,
            geometry.height,
            u32::MAX,
        )
        .map_err(|error| CoreError::runtime(format!("X11 get_image failed: {error}")))?
        .reply()
        .map_err(|error| CoreError::runtime(format!("X11 get_image reply failed: {error}")))?;

    let setup = connection.setup();
    let format = setup
        .pixmap_formats
        .iter()
        .find(|format| format.depth == image.depth)
        .ok_or_else(|| {
            CoreError::runtime(format!("No X11 pixmap format for depth {}", image.depth))
        })?;
    let visual = setup.roots[screen_num]
        .allowed_depths
        .iter()
        .flat_map(|depth| depth.visuals.iter())
        .find(|visual| visual.visual_id == image.visual)
        .ok_or_else(|| CoreError::runtime(format!("No X11 visual for id {}", image.visual)))?;
    let bytes_per_pixel = usize::from(format.bits_per_pixel.div_ceil(8));
    let row_bits = usize::from(geometry.width) * usize::from(format.bits_per_pixel);
    let pad = usize::from(format.scanline_pad);
    let stride = row_bits.div_ceil(pad) * pad / 8;
    let required = stride * usize::from(geometry.height);
    if image.data.len() < required || !(3..=4).contains(&bytes_per_pixel) {
        return Err(CoreError::runtime(format!(
            "Unsupported X11 image layout: depth={}, bitsPerPixel={}, bytes={}, required={required}",
            image.depth,
            format.bits_per_pixel,
            image.data.len()
        )));
    }

    let mut rgb =
        Vec::with_capacity(usize::from(geometry.width) * usize::from(geometry.height) * 3);
    for y in 0..usize::from(geometry.height) {
        for x in 0..usize::from(geometry.width) {
            let offset = y * stride + x * bytes_per_pixel;
            let bytes = &image.data[offset..offset + bytes_per_pixel];
            let pixel = match setup.image_byte_order {
                ImageOrder::LSB_FIRST => bytes
                    .iter()
                    .enumerate()
                    .fold(0_u32, |value, (index, byte)| {
                        value | (u32::from(*byte) << (index * 8))
                    }),
                ImageOrder::MSB_FIRST => bytes
                    .iter()
                    .fold(0_u32, |value, byte| (value << 8) | u32::from(*byte)),
                _ => return Err(CoreError::runtime("Unsupported X11 image byte order")),
            };
            rgb.extend_from_slice(&[
                component(pixel, visual.red_mask),
                component(pixel, visual.green_mask),
                component(pixel, visual.blue_mask),
            ]);
        }
    }

    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(
            &mut output,
            u32::from(geometry.width),
            u32::from(geometry.height),
        );
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| CoreError::runtime(format!("Cannot encode screenshot: {error}")))?;
        writer
            .write_image_data(&rgb)
            .map_err(|error| CoreError::runtime(format!("Cannot encode screenshot: {error}")))?;
    }
    Ok(output)
}

#[cfg(not(target_os = "linux"))]
fn host_window(_qmp_socket: &Path) -> CoreResult<Vec<u8>> {
    Err(CoreError::runtime(
        "Host-window screenshot fallback is supported on Linux only",
    ))
}

pub async fn capture(
    _transport: &mut EmulatorTransport,
    qmp: &mut QmpClient,
) -> CoreResult<Vec<u8>> {
    match screendump(qmp).await {
        Ok(data) => Ok(data),
        Err(qmp_error) => host_window(qmp.socket()).map_err(|host_error| {
            CoreError::runtime(format!(
                "Screenshot failed through QMP ({qmp_error}) and host window capture ({host_error})"
            ))
        }),
    }
}

pub fn save(data: &[u8], output: &Path) -> CoreResult<()> {
    std::fs::write(output, data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_png() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.resize(101, 0);
        assert!(valid_png(&bytes));
    }

    #[test]
    fn expands_color_masks() {
        assert_eq!(component(0x00ff_0000, 0x00ff_0000), 255);
        assert_eq!(component(0x0000_8000, 0x0000_ff00), 128);
        assert_eq!(component(0, 0x0000_00ff), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn matches_only_the_exact_qmp_socket() {
        assert!(qmp_argument_matches(
            "unix:/tmp/audb/qmp.sock,server=on,wait=off",
            "/tmp/audb/qmp.sock"
        ));
        assert!(!qmp_argument_matches(
            "unix:/tmp/audb/qmp.sock.other,server=on",
            "/tmp/audb/qmp.sock"
        ));
    }
}
