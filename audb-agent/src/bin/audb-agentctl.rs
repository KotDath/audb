use audb_agent::{read_frame, read_socket_frame, Action, Display, Request, PROTOCOL, SOCKET};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
static CANCELLED: AtomicBool = AtomicBool::new(false);
extern "C" fn cancel(_: libc::c_int) {
    CANCELLED.store(true, Ordering::Relaxed);
}
enum Output {
    Json(Value),
    Png(Vec<u8>, u32, u32),
}

type Failure = (&'static str, String);
fn unavailable(e: impl std::fmt::Display) -> Failure {
    ("AGENT_UNAVAILABLE", e.to_string())
}
fn invalid(e: impl std::fmt::Display) -> Failure {
    ("INVALID_ARGUMENT", e.to_string())
}
fn unknown(e: impl std::fmt::Display) -> Failure {
    (
        "OUTCOME_UNKNOWN",
        format!("Agent outcome unknown; command was not repeated: {e}"),
    )
}
fn display() -> Result<Display, Failure> {
    let mut child = Command::new("/usr/libexec/audb-agent/display")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(unavailable)?;
    let begin = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if begin.elapsed() < Duration::from_secs(3) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unavailable(match result {
                    Err(e) => e.to_string(),
                    _ => "Screen geometry query timed out".into(),
                }));
            }
        }
    };
    if !status.success() {
        return Err(unavailable(
            "Cannot query Aurora graphical session geometry",
        ));
    }
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .take(4096)
        .read_to_end(&mut bytes)
        .map_err(unavailable)?;
    let display: Display = serde_json::from_slice(&bytes).map_err(unavailable)?;
    display.validate().map_err(unavailable)?;
    Ok(display)
}
fn run() -> Result<Output, Failure> {
    let mut action: Action =
        serde_json::from_slice(&read_frame(std::io::stdin()).map_err(invalid)?).map_err(invalid)?;
    if matches!(action, Action::Text { .. } | Action::InputStatus) {
        return audb_agent::input::call(&action, || CANCELLED.load(Ordering::Relaxed))
            .map(Output::Json);
    }
    let status = matches!(action, Action::Status);
    let display = match &action {
        Action::Permission {
            application_id,
            action,
        } => {
            audb_agent::permission::validate(application_id, action)
                .map_err(|e| invalid(e.message))?;
            None
        }
        Action::PermissionCapabilities => None,
        Action::Key { name } => {
            if audb_protocol::input::key_code(name).is_none() {
                return Err(invalid("Unsupported key name"));
            }
            None
        }
        Action::Status => display().ok(),
        _ => {
            let geometry = display()?;
            audb_agent::gesture(geometry, &action).map_err(invalid)?;
            Some(geometry)
        }
    };
    let screenshot_directory = if let Action::Screenshot { directory } = &mut action {
        if directory.is_some() {
            return Err(invalid("Screenshot staging is managed by agentctl"));
        }
        let dir =
            audb_agent::screenshot::prepare(unsafe { libc::geteuid() }).map_err(unavailable)?;
        *directory = Some(
            dir.path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        Some(dir)
    } else {
        None
    };
    let request = Request {
        protocol_version: PROTOCOL,
        display,
        action,
    };
    let mut socket = UnixStream::connect(SOCKET).map_err(|e| {
        unavailable(format!(
            "audb-agent is unavailable: {e}; run audb setup-device"
        ))
    })?;
    socket
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(unavailable)?;
    let mut bytes = serde_json::to_vec(&request).map_err(invalid)?;
    bytes.push(b'\n');
    socket.write_all(&bytes).map_err(unknown)?;
    let response = read_socket_frame(&socket, Duration::from_secs(25)).map_err(unknown)?;
    let mut response: Value = serde_json::from_slice(&response).map_err(unknown)?;
    if status && response["ok"] == true {
        let input =
            audb_agent::input::call(&Action::InputStatus, || CANCELLED.load(Ordering::Relaxed))
                .ok();
        let available = input
            .as_ref()
            .is_some_and(|v| v["ok"] == true && v["data"]["available"] == true);
        response["data"]["capabilities"]["text"] = Value::Bool(available);
        response["data"]["input"] = input
            .map(|v| v["data"].clone())
            .unwrap_or(serde_json::json!({"available":false}));
    }
    if let Some(directory) = screenshot_directory {
        if response["ok"] != true {
            return Ok(Output::Json(response));
        }
        let (bytes, (width, height)) = audb_agent::screenshot::wait_png(
            &directory.path().join(audb_agent::screenshot::FILE),
            unsafe { libc::geteuid() },
            Duration::from_secs(7),
            || CANCELLED.load(Ordering::Relaxed),
        )
        .map_err(|e| ("SCREENSHOT_FAILED", e))?;
        // Remove remote artifacts before writing any image bytes to SSH stdout.
        directory.close().map_err(|e| {
            (
                "SCREENSHOT_FAILED",
                format!("Cannot clean screenshot staging: {e}"),
            )
        })?;
        return Ok(Output::Png(bytes, width, height));
    }
    Ok(Output::Json(response))
}
fn main() {
    unsafe {
        libc::signal(libc::SIGTERM, cancel as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, cancel as *const () as libc::sighandler_t);
    }
    match run() {
        Ok(Output::Json(v)) => println!("{v}"),
        Ok(Output::Png(bytes, width, height)) => {
            if let Err(e) = audb_agent::screenshot::write_binary(
                std::io::stdout().lock(),
                &bytes,
                width,
                height,
            ) {
                eprintln!("Cannot deliver screenshot: {e}");
                std::process::exit(1);
            }
        }
        Err((code, message)) => println!(
            "{}",
            serde_json::json!({"ok":false,"error":{"code":code,"message":message}})
        ),
    }
}
