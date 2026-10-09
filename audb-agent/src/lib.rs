use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Read};

pub const SOCKET: &str = "/run/audb-agent/control.sock";
pub const PROTOCOL: u32 = 3;
pub const MAX_FRAME: u64 = 65536;
pub const MAX_PNG: usize = 16 * 1024 * 1024;
pub mod input;
pub mod permission;
pub mod screenshot;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Display {
    pub native_width: i32,
    pub native_height: i32,
    pub rotation: i32,
}
impl Display {
    pub fn validate(self) -> Result<(), String> {
        if !(2..=16384).contains(&self.native_width)
            || !(2..=16384).contains(&self.native_height)
            || ![0, 90, 180, 270].contains(&self.rotation)
        {
            return Err("Invalid display geometry".into());
        }
        Ok(())
    }
    pub fn size(self) -> (i32, i32) {
        if self.rotation == 90 || self.rotation == 270 {
            (self.native_height, self.native_width)
        } else {
            (self.native_width, self.native_height)
        }
    }
    pub fn native_point(self, x: i32, y: i32) -> Result<(i32, i32), String> {
        self.validate()?;
        let (w, h) = self.size();
        if !(0..w).contains(&x) || !(0..h).contains(&y) {
            return Err(format!("Coordinates {x},{y} outside {w}x{h}"));
        }
        Ok(match self.rotation {
            90 => (self.native_width - 1 - y, x),
            180 => (self.native_width - 1 - x, self.native_height - 1 - y),
            270 => (y, self.native_height - 1 - x),
            _ => (x, y),
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    InputStatus,
    Text {
        text: String,
        delay_ms: u64,
    },
    Key {
        name: String,
    },
    PermissionCapabilities,
    Permission {
        application_id: String,
        action: audb_protocol::PermissionAction,
    },
    Status,
    Screenshot {
        // Filled by agentctl, never a host-supplied filesystem path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        directory: Option<String>,
    },
    Tap {
        x: i32,
        y: i32,
        duration_ms: u64,
    },
    Swipe {
        args: Vec<String>,
        duration_ms: Option<u64>,
        hold_ms: Option<u64>,
        steps: Option<u32>,
    },
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub protocol_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<Display>,
    pub action: Action,
}
#[derive(Debug)]
pub struct Gesture {
    pub start: (i32, i32),
    pub end: (i32, i32),
    pub duration: u64,
    pub hold: u64,
    pub steps: u32,
}
pub fn gesture(display: Display, action: &Action) -> Result<Option<Gesture>, String> {
    display.validate()?;
    let g = match action {
        Action::Status
        | Action::Text { .. }
        | Action::Key { .. }
        | Action::InputStatus
        | Action::Screenshot { .. }
        | Action::PermissionCapabilities
        | Action::Permission { .. } => return Ok(None),
        Action::Tap { x, y, duration_ms } => {
            if !(1..=3000).contains(duration_ms) {
                return Err("Tap duration must be 1..3000 ms".into());
            }
            let point = display.native_point(*x, *y)?;
            return Ok(Some(Gesture {
                start: point,
                end: point,
                duration: 0,
                hold: *duration_ms,
                steps: 0,
            }));
        }
        Action::Swipe {
            args,
            duration_ms,
            hold_ms,
            steps,
        } => {
            let (w, h) = display.size();
            let (coords, fast, long) = if args.len() == 4 {
                let v = args
                    .iter()
                    .map(|s| s.parse::<i32>())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| "Swipe requires a direction or X1 Y1 X2 Y2")?;
                ((v[0], v[1], v[2], v[3]), false, false)
            } else if args.len() == 1 {
                let d = args[0].as_str();
                let fast = d.starts_with("fast-") || d.starts_with("edge-");
                let long = d.starts_with("long-");
                let d = d
                    .strip_prefix("fast-")
                    .or_else(|| d.strip_prefix("long-"))
                    .unwrap_or(d);
                let coords = match d {
                    "up" => (w / 2, h * 78 / 100, w / 2, h * 22 / 100),
                    "down" => (w / 2, h * 22 / 100, w / 2, h * 78 / 100),
                    "left" => (w * 90 / 100, h / 2, w / 10, h / 2),
                    "right" => (w / 10, h / 2, w * 90 / 100, h / 2),
                    "edge-up" => (w / 2, h - 3, w / 2, h / 8),
                    "edge-down" => (w / 2, 3, w / 2, h - h / 8),
                    "edge-left" => (w - 3, h / 2, w / 8, h / 2),
                    "edge-right" => (3, h / 2, w - w / 8, h / 2),
                    _ => return Err(format!("Unknown swipe direction: {d}")),
                };
                (coords, fast, long)
            } else {
                return Err("Swipe requires a direction or X1 Y1 X2 Y2".into());
            };
            let duration = duration_ms.unwrap_or(if long {
                1500
            } else if fast {
                500
            } else {
                900
            });
            let hold = hold_ms.unwrap_or(if fast || long { 50 } else { 160 });
            let steps = steps.unwrap_or(60);
            if !(40..=3000).contains(&duration) || hold > 1000 || !(1..=240).contains(&steps) {
                return Err(
                    "Swipe duration must be 40..3000 ms, hold 0..1000 ms, steps 1..240".into(),
                );
            }
            Gesture {
                start: display.native_point(coords.0, coords.1)?,
                end: display.native_point(coords.2, coords.3)?,
                duration,
                hold,
                steps,
            }
        }
    };
    Ok(Some(g))
}
pub fn read_frame(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut reader = BufReader::new(reader.take(MAX_FRAME + 1));
    let mut bytes = Vec::new();
    reader.read_until(b'\n', &mut bytes)?;
    if bytes.len() as u64 > MAX_FRAME || bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid or oversized agent frame",
        ));
    }
    Ok(bytes)
}

/// Absolute frame deadline prevents slow clients from extending the timeout.
pub fn read_socket_frame(
    socket: &std::os::unix::net::UnixStream,
    timeout: std::time::Duration,
) -> io::Result<Vec<u8>> {
    read_socket_frame_cancellable(socket, timeout, || false)
}
pub fn read_socket_frame_cancellable(
    socket: &std::os::unix::net::UnixStream,
    timeout: std::time::Duration,
    cancelled: impl Fn() -> bool,
) -> io::Result<Vec<u8>> {
    let deadline = std::time::Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut reader = socket;
    loop {
        if cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Agent request cancelled",
            ));
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Agent frame deadline exceeded",
            ));
        }
        socket.set_read_timeout(Some(remaining.min(std::time::Duration::from_millis(100))))?;
        let mut chunk = [0u8; 1024];
        let n = match reader.read(&mut chunk) {
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue
            }
            result => result?,
        };
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Agent connection closed",
            ));
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() as u64 > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Oversized agent frame",
            ));
        }
        if let Some(index) = bytes.iter().position(|b| *b == b'\n') {
            if index + 1 != bytes.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Only one command per connection is allowed",
                ));
            }
            return Ok(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_reply_wait_closes_without_waiting_for_input_deadline() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let (server, _client) = std::os::unix::net::UnixStream::pair().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            signal.store(true, Ordering::Relaxed);
        });
        let start = std::time::Instant::now();
        let error =
            read_socket_frame_cancellable(&server, std::time::Duration::from_secs(13), || {
                cancelled.load(Ordering::Relaxed)
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        thread.join().unwrap();
    }
    #[test]
    fn all_rotations_and_bounds() {
        for (rotation, expected) in [
            (0, (199, 299)),
            (90, (900, 199)),
            (180, (1000, 1700)),
            (270, (299, 1800)),
        ] {
            let d = Display {
                native_width: 1200,
                native_height: 2000,
                rotation,
            };
            assert_eq!(d.native_point(199, 299).unwrap(), expected);
            let (w, h) = d.size();
            assert!(d.native_point(w, 0).is_err());
            assert!(d.native_point(0, h).is_err());
            assert!(d.native_point(-1, 0).is_err());
        }
    }
    #[test]
    fn gesture_validation() {
        let d = Display {
            native_width: 1200,
            native_height: 2000,
            rotation: 90,
        };
        let g = gesture(
            d,
            &Action::Swipe {
                args: vec!["edge-right".into()],
                duration_ms: None,
                hold_ms: None,
                steps: None,
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(g.start, (599, 3));
        assert_eq!(g.end, (599, 1750));
        assert!(gesture(
            d,
            &Action::Tap {
                x: 2000,
                y: 0,
                duration_ms: 80
            }
        )
        .is_err());
        assert!(gesture(
            d,
            &Action::Swipe {
                args: vec!["up".into()],
                duration_ms: Some(u64::MAX),
                hold_ms: None,
                steps: None
            }
        )
        .is_err());
    }
    #[test]
    fn socket_deadline_and_extra_frames_are_rejected() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;
        use std::time::{Duration, Instant};
        let (server, mut client) = UnixStream::pair().unwrap();
        client.write_all(b"{}\n{}\n").unwrap();
        assert_eq!(
            read_socket_frame(&server, Duration::from_millis(30))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let (server, _client) = UnixStream::pair().unwrap();
        let start = Instant::now();
        assert!(read_socket_frame(&server, Duration::from_millis(30)).is_err());
        assert!(start.elapsed() < Duration::from_millis(300));
    }
    #[test]
    fn frame_limits() {
        assert!(read_frame(&b"{}\n"[..]).is_ok());
        assert!(read_frame(&b"{}"[..]).is_err());
        assert!(read_frame(vec![b'a'; MAX_FRAME as usize + 2].as_slice()).is_err());
    }
}
