//! Lipstick captures to the calling graphical user's private staging directory.
//! The service only requests capture; it never reads user files as root.
use crate::MAX_PNG;
use std::ffi::CStr;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const PREFIX: &str = "audb-screenshot-";
pub const FILE: &str = "screen.png";

pub fn user_home(uid: libc::uid_t) -> Result<PathBuf, String> {
    // Both callers are single-threaded; copy libc's static storage immediately.
    let entry = unsafe { libc::getpwuid(uid) };
    if entry.is_null() {
        return Err(format!("No passwd entry for graphical user {uid}"));
    }
    let home = unsafe { CStr::from_ptr((*entry).pw_dir) }
        .to_str()
        .map_err(|e| e.to_string())?;
    let path = PathBuf::from(home);
    if !path.is_absolute() {
        return Err("User home must be absolute".into());
    }
    Ok(path)
}
pub fn capture_path(home: &Path, directory: &str) -> Result<PathBuf, String> {
    let suffix = directory
        .strip_prefix(PREFIX)
        .ok_or("Invalid screenshot directory")?;
    if !(6..=64).contains(&suffix.len()) || !suffix.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("Invalid screenshot directory token".into());
    }
    Ok(home.join("Pictures/Screenshots").join(directory).join(FILE))
}
pub fn prepare(uid: libc::uid_t) -> Result<tempfile::TempDir, String> {
    let parent = user_home(uid)?.join("Pictures/Screenshots");
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    tempfile::Builder::new()
        .prefix(PREFIX)
        .tempdir_in(parent)
        .map_err(|e| e.to_string())
}
/// Privileged request, without a shell or arbitrary destination path.
pub fn request(uid: libc::uid_t, directory: &str) -> Result<(), String> {
    let path = capture_path(&user_home(uid)?, directory)?;
    let bus_path = PathBuf::from(format!("/run/user/{uid}/dbus/user_bus_socket"));
    if !bus_path.exists() {
        return Err(format!("No Aurora graphical session bus for uid {uid}"));
    }
    let mut child = Command::new("/usr/bin/dbus-send")
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path={}", bus_path.display()),
        )
        .args([
            "--session",
            "--print-reply",
            "--reply-timeout=3000",
            "--dest=org.nemomobile.lipstick",
            "/org/nemomobile/lipstick/screenshot",
            "org.nemomobile.lipstick.saveScreenshot",
        ])
        .arg(format!("string:{}", path.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let begin = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(());
                }
                let mut stderr = String::new();
                if let Some(pipe) = child.stderr.take() {
                    let _ = pipe.take(4096).read_to_string(&mut stderr);
                }
                return Err(format!(
                    "Lipstick screenshot request failed: {}",
                    stderr.trim()
                ));
            }
            Ok(None) if begin.elapsed() < Duration::from_secs(4) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(e) => e.to_string(),
                    _ => "Lipstick screenshot request timed out".into(),
                });
            }
        }
    }
}
/// Validate complete PNG chunks, CRCs, IHDR geometry and final IEND. A truncated
/// or still-being-written PNG is never considered ready just because it exists.
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() > MAX_PNG || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let mut offset = 8usize;
    let mut dimensions = None;
    let mut data = false;
    while offset.checked_add(12)? <= bytes.len() {
        let len = u32::from_be_bytes(bytes[offset..offset + 4].try_into().ok()?) as usize;
        let end = offset.checked_add(12)?.checked_add(len)?;
        if end > bytes.len() {
            return None;
        }
        let kind = &bytes[offset + 4..offset + 8];
        let payload = &bytes[offset + 8..end - 4];
        let crc = u32::from_be_bytes(bytes[end - 4..end].try_into().ok()?);
        if crc32fast::hash(&bytes[offset + 4..end - 4]) != crc {
            return None;
        }
        if offset == 8 {
            if kind != b"IHDR" || len != 13 {
                return None;
            }
            let width = u32::from_be_bytes(payload[..4].try_into().ok()?);
            let height = u32::from_be_bytes(payload[4..8].try_into().ok()?);
            if width == 0 || height == 0 || width > 16384 || height > 16384 {
                return None;
            }
            dimensions = Some((width, height));
        } else if kind == b"IHDR" {
            return None;
        }
        if kind == b"IDAT" {
            data = true;
        }
        if kind == b"IEND" {
            return if len == 0 && data && end == bytes.len() {
                dimensions
            } else {
                None
            };
        }
        offset = end;
    }
    None
}
pub fn wait_png(
    path: &Path,
    uid: libc::uid_t,
    timeout: Duration,
    cancelled: impl Fn() -> bool,
) -> Result<(Vec<u8>, (u32, u32)), String> {
    let begin = Instant::now();
    while begin.elapsed() < timeout && !cancelled() {
        match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
        {
            Ok(file) => {
                let metadata = file.metadata().map_err(|e| e.to_string())?;
                if !metadata.is_file() || metadata.uid() != uid {
                    return Err(
                        "Screenshot is not a regular file owned by the graphical user".into(),
                    );
                }
                if metadata.len() > MAX_PNG as u64 {
                    return Err("Screenshot exceeds 16 MiB limit".into());
                }
                let mut bytes = Vec::new();
                file.take((MAX_PNG + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if let Some(dimensions) = png_dimensions(&bytes) {
                    return Ok((bytes, dimensions));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Cannot read screenshot: {e}")),
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    if cancelled() {
        Err("Screenshot cancelled".into())
    } else {
        Err("Timed out waiting for a complete Lipstick PNG".into())
    }
}
/// Header + PNG to the host, separate from the JSON-only local service protocol.
pub fn write_binary(
    mut out: impl Write,
    bytes: &[u8],
    width: u32,
    height: u32,
) -> std::io::Result<()> {
    let header = serde_json::json!({"ok":true,"data":{"format":"png","bytes":bytes.len(),"width":width,"height":height}});
    writeln!(out, "{header}")?;
    out.write_all(bytes)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    const PNG: &[u8] = include_bytes!("../tests/one-pixel.png");
    #[test]
    fn rejects_incomplete_corrupt_and_trailing_png() {
        assert_eq!(png_dimensions(PNG), Some((1, 1)));
        assert_eq!(png_dimensions(&PNG[..PNG.len() - 1]), None);
        let mut corrupt = PNG.to_vec();
        corrupt[30] ^= 1;
        assert_eq!(png_dimensions(&corrupt), None);
        let mut trailing = PNG.to_vec();
        trailing.push(0);
        assert_eq!(png_dimensions(&trailing), None);
    }
    #[test]
    fn waits_for_complete_file_and_refuses_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        std::fs::write(&path, &PNG[..20]).unwrap();
        let other = path.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            std::fs::write(other, PNG).unwrap();
        });
        let (data, size) = wait_png(
            &path,
            unsafe { libc::geteuid() },
            Duration::from_millis(500),
            || false,
        )
        .unwrap();
        writer.join().unwrap();
        assert_eq!(data, PNG);
        assert_eq!(size, (1, 1));
        let symlink = dir.path().join("link.png");
        std::os::unix::fs::symlink(&path, &symlink).unwrap();
        assert!(wait_png(
            &symlink,
            unsafe { libc::geteuid() },
            Duration::from_millis(30),
            || false
        )
        .is_err());
        assert!(wait_png(
            &path,
            unsafe { libc::geteuid() },
            Duration::from_millis(30),
            || true
        )
        .is_err());
    }
    #[test]
    fn destinations_cannot_escape_user_staging() {
        let home = Path::new("/home/graphical");
        assert!(capture_path(home, "audb-screenshot-abc123")
            .unwrap()
            .starts_with(home));
        for token in [
            "../elsewhere",
            "audb-screenshot-../../root",
            "audb-screenshot-.hidden",
            "audb-screenshot-a/b",
            "audb-screenshot-",
        ] {
            assert!(capture_path(home, token).is_err());
        }
    }
    #[test]
    fn cleanup_after_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_owned();
        assert!(wait_png(
            &path.join(FILE),
            unsafe { libc::geteuid() },
            Duration::from_millis(30),
            || false
        )
        .is_err());
        drop(dir);
        assert!(!path.exists());
    }
}
