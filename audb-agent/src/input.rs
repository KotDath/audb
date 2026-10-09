//! Unprivileged client for the narrow, per-user Maliit input socket.
use crate::{read_socket_frame_cancellable, Action};
use serde_json::Value;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub fn call(
    action: &Action,
    cancelled: impl Fn() -> bool,
) -> Result<Value, (&'static str, String)> {
    if let Action::Text { text, delay_ms } = action {
        audb_protocol::input::validate_text(text, *delay_ms)
            .map_err(|e| ("INVALID_ARGUMENT", e))?;
    }
    let uid = unsafe { libc::geteuid() };
    let path = format!("/run/user/{uid}/audb-input.sock");
    let mut socket = UnixStream::connect(path).map_err(|e| (
        "CAPABILITY_UNAVAILABLE", format!("Maliit input bridge unavailable: {e}; upgrade audb-agent and restart the user maliit-server service")))?;
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut size,
        )
    } != 0
        || cred.uid != uid
    {
        return Err((
            "PERMISSION_DENIED",
            "Input socket is not owned by this user".into(),
        ));
    }
    socket
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| ("CAPABILITY_UNAVAILABLE", e.to_string()))?;
    let mut bytes = serde_json::to_vec(action).map_err(|e| ("INVALID_ARGUMENT", e.to_string()))?;
    bytes.push(b'\n');
    socket
        .write_all(&bytes)
        .map_err(|e| ("OUTCOME_UNKNOWN", e.to_string()))?;
    let bytes = read_socket_frame_cancellable(&socket, Duration::from_secs(13), cancelled)
        .map_err(|e| {
            (
                "OUTCOME_UNKNOWN",
                format!("Text outcome unknown; not repeated: {e}"),
            )
        })?;
    serde_json::from_slice(&bytes).map_err(|e| ("OUTCOME_UNKNOWN", e.to_string()))
}
