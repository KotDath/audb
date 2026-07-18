use crate::config::EmulatorConfig;
use crate::error::{CoreError, CoreResult};
use serde_json::{json, Value};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tokio::net::UnixStream;

pub async fn is_running(config: &EmulatorConfig) -> bool {
    tokio::time::timeout(
        Duration::from_secs(2),
        UnixStream::connect(&config.qmp_socket),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

fn sfdk(
    config: &EmulatorConfig,
    args: &[&str],
    timeout: Duration,
    detached: bool,
) -> CoreResult<String> {
    let mut command = Command::new(config.sfdk());
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if detached {
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| CoreError::runtime(format!("Cannot run sfdk: {e}")))?;
    let start = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|e| CoreError::runtime(e.to_string()))?
            .is_some()
        {
            break;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            return Err(CoreError::runtime(format!(
                "sfdk {} timed out",
                args.join(" ")
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let output = child
        .wait_with_output()
        .map_err(|e| CoreError::runtime(e.to_string()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() && (stderr.contains("Error") || stderr.contains("error:")) {
        return Err(CoreError::runtime(format!(
            "sfdk {} failed: {stderr}",
            args.join(" ")
        )));
    }
    Ok(stdout)
}

pub async fn start(config: &EmulatorConfig, timeout: Duration) -> CoreResult<Value> {
    if is_running(config).await {
        return Ok(
            json!({"running": true, "alreadyRunning": true, "qmpSocket": config.qmp_socket}),
        );
    }
    let mut launcher = Command::new(config.sfdk())
        .args(["emulator", "start"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| CoreError::runtime(format!("Cannot run sfdk: {e}")))?;
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if is_running(config).await {
            if launcher.try_wait().ok().flatten().is_none() {
                // Some SDK versions keep the launcher attached to the running QEMU. QMP is
                // the authoritative readiness signal, so do not retain that waiter forever.
                let _ = launcher.kill();
            }
            return Ok(
                json!({"running": true, "alreadyRunning": false, "qmpSocket": config.qmp_socket}),
            );
        }
        if let Some(status) = launcher
            .try_wait()
            .map_err(|e| CoreError::runtime(e.to_string()))?
        {
            if !status.success() {
                return Err(CoreError::runtime(format!(
                    "sfdk emulator start failed with {status}"
                )));
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let _ = launcher.kill();
    Err(CoreError::new(
        audb_protocol::ErrorCode::EmulatorOff,
        format!("QMP socket not ready: {}", config.qmp_socket.display()),
    ))
}

pub async fn stop(config: &EmulatorConfig, timeout: Duration) -> CoreResult<Value> {
    sfdk(config, &["emulator", "stop"], timeout, false)?;
    Ok(json!({"stopRequested": true}))
}

pub async fn status(config: &EmulatorConfig) -> Value {
    json!({"running": is_running(config).await, "qmpSocket": config.qmp_socket})
}
