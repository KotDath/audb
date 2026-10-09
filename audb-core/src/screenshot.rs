use crate::error::{CoreError, CoreResult};
use crate::qmp::QmpClient;
use crate::transport::{shell_quote, DeviceTransport};
use audb_protocol::ErrorCode;
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Read the primary QMP display's current dimensions, never a host window or
/// guest Qt geometry. Only the PNG header/trailer are read into memory.
pub(crate) async fn qmp_geometry(qmp: &mut QmpClient) -> CoreResult<(u32, u32)> {
    let directory = tempfile::Builder::new()
        .prefix("audb-qmp-geometry-")
        .tempdir()?;
    let path = directory.path().join("screen.png");
    qmp.execute("screendump", Some(json!({"filename":path,"format":"png"})))
        .await?;
    let mut file = tokio::fs::File::open(&path).await?;
    let mut edges = [0u8; 45];
    file.read_exact(&mut edges[..33]).await?;
    file.seek(std::io::SeekFrom::End(-12)).await?;
    file.read_exact(&mut edges[33..]).await?;
    png_dimensions(&edges)
        .ok_or_else(|| CoreError::runtime("QMP geometry capture is not a valid PNG"))
}

pub fn png_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 45
        || !data.starts_with(b"\x89PNG\r\n\x1a\n")
        || data[8..16] != *b"\0\0\0\rIHDR"
        || !data.ends_with(b"\0\0\0\0IEND\xaeB`\x82")
    {
        return None;
    }
    let w = u32::from_be_bytes(data[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(data[20..24].try_into().ok()?);
    (w > 0 && h > 0 && w <= 16384 && h <= 16384).then_some((w, h))
}
fn valid_png(data: &[u8]) -> bool {
    png_dimensions(data).is_some()
}

/// Physical screenshots use the installed agent, never root SSH credentials.
pub async fn capture_physical(transport: &mut DeviceTransport) -> CoreResult<Vec<u8>> {
    let status = crate::physical_input::call(transport, json!({"command":"status"})).await?;
    if status["capabilities"]["screenshot"] != true {
        return Err(CoreError::new(
            ErrorCode::CapabilityUnavailable,
            "Installed audb-agent does not support screenshots; upgrade it using setup-device",
        ));
    }
    let mut request = serde_json::to_vec(&json!({"command":"screenshot"}))?;
    request.push(b'\n');
    let bytes = transport
        .exec_stdin_bytes(crate::physical_input::AGENT_COMMAND, request)
        .await?;
    parse_physical(&bytes)
}
fn parse_physical(bytes: &[u8]) -> CoreResult<Vec<u8>> {
    let newline = bytes
        .iter()
        .position(|b| *b == b'\n')
        .filter(|index| *index <= 8192)
        .ok_or_else(|| CoreError::runtime("Invalid screenshot response header"))?;
    let header = &bytes[..newline];
    let body = &bytes[newline + 1..];
    let text = std::str::from_utf8(header)
        .map_err(|_| CoreError::runtime("Screenshot response header is not UTF-8"))?;
    let data = crate::physical_input::parse(text)?;
    let length = data["bytes"]
        .as_u64()
        .filter(|len| *len <= 16 * 1024 * 1024)
        .ok_or_else(|| CoreError::runtime("Invalid screenshot size in agent response"))?;
    let dimensions = png_dimensions(body)
        .ok_or_else(|| CoreError::runtime("Agent returned an incomplete or invalid PNG"))?;
    if data["format"] != "png"
        || length != body.len() as u64
        || data["width"] != dimensions.0
        || data["height"] != dimensions.1
    {
        return Err(CoreError::runtime(
            "Screenshot payload does not match agent metadata",
        ));
    }
    Ok(body.to_vec())
}

async fn lipstick(transport: &mut DeviceTransport) -> CoreResult<Vec<u8>> {
    let user = transport.config().ssh_user.clone();
    let token = format!(
        "{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    );
    let remote = PathBuf::from(format!("/home/{user}/Pictures/audb-screenshot-{token}.png"));
    let quoted_user = shell_quote(&user);
    let quoted_remote = shell_quote(&remote.to_string_lossy());
    let command = format!(
        "uid=$(id -u {quoted_user}) && export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$uid/dbus/user_bus_socket && \
         gdbus call --session --dest org.nemomobile.lipstick --object-path /org/nemomobile/lipstick/screenshot \
         --method org.nemomobile.lipstick.saveScreenshot {quoted_remote} >/dev/null && \
         for i in $(seq 1 40); do test -s {quoted_remote} && exit 0; sleep 0.05; done; exit 1"
    );
    let result = async {
        transport.exec(&command, false).await?;
        let data = transport.download_bytes(&remote).await?;
        if !valid_png(&data) {
            return Err(CoreError::runtime("Lipstick returned an invalid PNG"));
        }
        Ok(data)
    }
    .await;
    let _ = transport
        .exec(&format!("rm -f {quoted_remote}"), false)
        .await;
    result
}

async fn screendump(qmp: &mut QmpClient) -> CoreResult<Vec<u8>> {
    let directory = tempfile::Builder::new()
        .prefix("audb-qmp-shot-")
        .tempdir()?;
    let path = directory.path().join("screen.png");
    qmp.execute(
        "screendump",
        Some(json!({"filename": path, "format": "png"})),
    )
    .await?;
    let data = tokio::fs::read(&path)
        .await
        .map_err(|e| CoreError::runtime(format!("Cannot read QMP screenshot: {e}")))?;
    if !valid_png(&data) {
        return Err(CoreError::runtime("QMP returned an invalid PNG"));
    }
    Ok(data)
}

fn host_window(transport: &DeviceTransport) -> CoreResult<Vec<u8>> {
    let name = &transport
        .config()
        .emulator
        .as_ref()
        .ok_or_else(|| CoreError::runtime("Host window capture requires emulator"))?
        .emulator_name;
    let output = Command::new("xdotool")
        .args(["search", "--name", name])
        .output()
        .map_err(|e| CoreError::runtime(format!("xdotool not available: {e}")))?;
    let id = String::from_utf8_lossy(&output.stdout)
        .lines()
        .last()
        .unwrap_or_default()
        .trim()
        .to_string();
    if id.is_empty() {
        return Err(CoreError::runtime(format!("QEMU window not found: {name}")));
    }
    let directory = tempfile::Builder::new()
        .prefix("audb-host-shot-")
        .tempdir()?;
    let path = directory.path().join("screen.png");
    let status = Command::new("import")
        .args(["-window", &id, path.to_str().unwrap_or_default()])
        .status()
        .map_err(|e| CoreError::runtime(format!("import not available: {e}")))?;
    if !status.success() {
        return Err(CoreError::runtime("import -window failed"));
    }
    let data = std::fs::read(&path)
        .map_err(|e| CoreError::runtime(format!("Cannot read host screenshot: {e}")))?;
    if !valid_png(&data) {
        return Err(CoreError::runtime("Host screenshot is not a PNG"));
    }
    Ok(data)
}

pub async fn capture(transport: &mut DeviceTransport, qmp: &mut QmpClient) -> CoreResult<Vec<u8>> {
    let mut errors = Vec::new();
    match screendump(qmp).await {
        Ok(data) => return Ok(data),
        Err(error) => errors.push(format!("QMP: {error}")),
    }
    match host_window(transport) {
        Ok(data) => return Ok(data),
        Err(error) => errors.push(format!("host: {error}")),
    }
    match lipstick(transport).await {
        Ok(data) => return Ok(data),
        Err(error) => errors.push(format!("Lipstick: {error}")),
    }
    Err(CoreError::runtime(format!(
        "Screenshot failed ({})",
        errors.join("; ")
    )))
}

pub fn save(data: &[u8], output: &Path) -> CoreResult<()> {
    if !valid_png(data) {
        return Err(CoreError::runtime("Cannot save an incomplete PNG"));
    }
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(data)?;
    file.as_file().sync_all()?;
    file.persist(output)
        .map_err(|e| CoreError::runtime(format!("Cannot save screenshot: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const PNG: &[u8] = include_bytes!("../../audb-agent/tests/one-pixel.png");
    #[test]
    fn recognizes_complete_png() {
        assert!(valid_png(PNG));
        assert!(!valid_png(&PNG[..PNG.len() - 1]));
        assert!(!valid_png(b"not an image"));
    }
    #[test]
    fn invalid_capture_does_not_replace_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("screen.png");
        save(PNG, &path).unwrap();
        assert!(save(&PNG[..PNG.len() - 1], &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), PNG);
    }
    #[test]
    fn physical_binary_contract_validates_size_and_errors() {
        let mut framed = serde_json::to_vec(
            &json!({"ok":true,"data":{"format":"png","bytes":PNG.len(),"width":1,"height":1}}),
        )
        .unwrap();
        framed.push(b'\n');
        framed.extend_from_slice(PNG);
        assert_eq!(parse_physical(&framed).unwrap(), PNG);
        assert!(parse_physical(&framed[..framed.len() - 1]).is_err());
        framed.push(0);
        assert!(parse_physical(&framed).is_err());
        assert_eq!(parse_physical(b"{\"ok\":false,\"error\":{\"code\":\"AGENT_UNAVAILABLE\",\"message\":\"setup\"}}\n").unwrap_err().code,ErrorCode::CapabilityUnavailable);
    }
}
