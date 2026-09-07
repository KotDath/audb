use crate::error::{CoreError, CoreResult};
use crate::qmp::QmpClient;
use crate::transport::{shell_quote, EmulatorTransport};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

fn valid_png(data: &[u8]) -> bool {
    data.len() > 100 && data.starts_with(b"\x89PNG\r\n\x1a\n")
}

async fn lipstick(transport: &mut EmulatorTransport) -> CoreResult<Vec<u8>> {
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
    let _ = tokio::fs::remove_file(&path).await;
    if !valid_png(&data) {
        return Err(CoreError::runtime("QMP returned an invalid PNG"));
    }
    Ok(data)
}

fn host_window(transport: &EmulatorTransport) -> CoreResult<Vec<u8>> {
    let name = &transport.config().emulator_name;
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
    let path = std::env::temp_dir().join(format!("audb-host-shot-{}.png", std::process::id()));
    let status = Command::new("import")
        .args(["-window", &id, path.to_str().unwrap_or_default()])
        .status()
        .map_err(|e| CoreError::runtime(format!("import not available: {e}")))?;
    if !status.success() {
        return Err(CoreError::runtime("import -window failed"));
    }
    let data = std::fs::read(&path)
        .map_err(|e| CoreError::runtime(format!("Cannot read host screenshot: {e}")))?;
    let _ = std::fs::remove_file(path);
    if !valid_png(&data) {
        return Err(CoreError::runtime("Host screenshot is not a PNG"));
    }
    Ok(data)
}

pub async fn capture(
    transport: &mut EmulatorTransport,
    qmp: &mut QmpClient,
) -> CoreResult<Vec<u8>> {
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
}
