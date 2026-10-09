//! Privileged, narrow input service. No shell execution or arbitrary file API.
use audb_agent::{gesture, read_socket_frame, Display, Gesture, Request, PROTOCOL, SOCKET};
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const BTN_TOUCH: u16 = 330;
const SLOT: u16 = 47;
const TRACK: u16 = 57;
const X: u16 = 53;
const Y: u16 = 54;
#[repr(C)]
struct InputEvent {
    time: libc::timeval,
    kind: u16,
    code: u16,
    value: i32,
}
#[repr(C)]
struct UinputDevice {
    name: [u8; 80],
    id: [u16; 4],
    ff_effects_max: u32,
    max: [i32; 64],
    min: [i32; 64],
    fuzz: [i32; 64],
    flat: [i32; 64],
}
struct Touch {
    file: File,
    active: bool,
    tracking: i32,
    display: Display,
}
impl Touch {
    fn create(display: Display) -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/uinput")?;
        let fd = file.as_raw_fd();
        for (request, arg) in [
            (0x40045564, EV_SYN as i32),
            (0x40045564, EV_KEY as i32),
            (0x40045564, EV_ABS as i32),
            (0x40045565, BTN_TOUCH as i32),
            (0x4004556e, 1),
        ] {
            if unsafe { libc::ioctl(fd, request as libc::c_ulong, arg) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let mut dev: UinputDevice = unsafe { std::mem::zeroed() };
        let name = b"audb-touchscreen";
        dev.name[..name.len()].copy_from_slice(name);
        dev.id = [6, 0x1234, 1, 1];
        for (code, max) in [
            (0, display.native_width - 1),
            (1, display.native_height - 1),
            (SLOT, 9),
            (TRACK, 65535),
            (X, display.native_width - 1),
            (Y, display.native_height - 1),
            (48, 255),
            (50, 255),
        ] {
            dev.max[code as usize] = max;
            if unsafe { libc::ioctl(fd, 0x40045567 as libc::c_ulong, code as i32) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&dev as *const UinputDevice).cast::<u8>(),
                std::mem::size_of::<UinputDevice>(),
            )
        };
        file.write_all(bytes)?;
        if unsafe { libc::ioctl(fd, 0x5501 as libc::c_ulong) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            file,
            active: false,
            tracking: 100,
            display,
        })
    }
    fn event(&mut self, kind: u16, code: u16, value: i32) -> io::Result<()> {
        let ev = InputEvent {
            time: libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            kind,
            code,
            value,
        };
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&ev as *const InputEvent).cast::<u8>(),
                std::mem::size_of::<InputEvent>(),
            )
        };
        self.file.write_all(bytes)
    }
    fn position(&mut self, p: (i32, i32)) -> io::Result<()> {
        self.event(EV_ABS, X, p.0)?;
        self.event(EV_ABS, Y, p.1)?;
        self.event(EV_ABS, 48, 19)?;
        self.event(EV_ABS, 50, 19)?;
        self.event(EV_SYN, 0, 0)
    }
    fn down(&mut self, p: (i32, i32)) -> io::Result<()> {
        self.active = true;
        self.tracking = self.tracking % 65535 + 1;
        self.event(EV_ABS, SLOT, 0)?;
        self.event(EV_ABS, TRACK, self.tracking)?;
        self.event(EV_ABS, X, p.0)?;
        self.event(EV_ABS, Y, p.1)?;
        self.event(EV_ABS, 48, 19)?;
        self.event(EV_ABS, 50, 19)?;
        self.event(EV_KEY, BTN_TOUCH, 1)?;
        self.event(EV_SYN, 0, 0)
    }
    fn up(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        // Attempt every release event, even when a prior write failed.
        let a = self.event(EV_KEY, BTN_TOUCH, 0);
        let b = self.event(EV_ABS, TRACK, -1);
        let c = self.event(EV_SYN, 0, 0);
        self.active = false;
        a.and(b).and(c)
    }
    fn perform(&mut self, g: Gesture, socket: &UnixStream) -> Result<(), String> {
        let result = (|| -> io::Result<()> {
            self.down(g.start)?;
            pause(Duration::from_millis(g.hold), socket)?;
            let begin = Instant::now();
            for step in 1..=g.steps {
                let due = Duration::from_millis(g.duration * step as u64 / g.steps as u64);
                pause(due.saturating_sub(begin.elapsed()), socket)?;
                let x = g.start.0
                    + ((g.end.0 - g.start.0) as i64 * step as i64 / g.steps as i64) as i32;
                let y = g.start.1
                    + ((g.end.1 - g.start.1) as i64 * step as i64 / g.steps as i64) as i32;
                self.position((x, y))?;
            }
            Ok(())
        })();
        let release = self.up();
        result.and(release).map_err(|e| e.to_string())
    }
}
impl Drop for Touch {
    fn drop(&mut self) {
        let _ = self.up();
        unsafe {
            libc::ioctl(self.file.as_raw_fd(), 0x5502 as libc::c_ulong);
        }
    }
}
// Persistent keyboard; every click releases the key even after disconnect.
struct Keyboard {
    file: File,
    pressed: Option<u16>,
}
impl Keyboard {
    fn create() -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/uinput")?;
        let fd = file.as_raw_fd();
        for event in [EV_SYN, EV_KEY] {
            if unsafe { libc::ioctl(fd, 0x40045564 as libc::c_ulong, event as i32) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        // Evdev requires ordinary keyboard keys to classify this as a keyboard.
        for key in 1..=116 {
            if unsafe { libc::ioctl(fd, 0x40045565 as libc::c_ulong, key) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let mut dev: UinputDevice = unsafe { std::mem::zeroed() };
        let name = b"audb-keyboard";
        dev.name[..name.len()].copy_from_slice(name);
        dev.id = [6, 0x1234, 2, 1];
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&dev as *const UinputDevice).cast::<u8>(),
                std::mem::size_of::<UinputDevice>(),
            )
        };
        file.write_all(bytes)?;
        if unsafe { libc::ioctl(fd, 0x5501 as libc::c_ulong) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            file,
            pressed: None,
        })
    }
    fn event(&mut self, kind: u16, code: u16, value: i32) -> io::Result<()> {
        let ev = InputEvent {
            time: libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            kind,
            code,
            value,
        };
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&ev as *const InputEvent).cast::<u8>(),
                std::mem::size_of::<InputEvent>(),
            )
        };
        self.file.write_all(bytes)
    }
    fn release(&mut self) -> io::Result<()> {
        if let Some(code) = self.pressed.take() {
            let key = self.event(EV_KEY, code, 0);
            let sync = self.event(EV_SYN, 0, 0);
            key.and(sync)
        } else {
            Ok(())
        }
    }
    fn click(&mut self, code: u16, socket: &UnixStream) -> io::Result<()> {
        self.pressed = Some(code);
        let result = (|| {
            self.event(EV_KEY, code, 1)?;
            self.event(EV_SYN, 0, 0)?;
            pause(Duration::from_millis(80), socket)
        })();
        let release = self.release();
        result.and(release)
    }
}
impl Drop for Keyboard {
    fn drop(&mut self) {
        let _ = self.release();
        unsafe {
            libc::ioctl(self.file.as_raw_fd(), 0x5502 as libc::c_ulong);
        }
    }
}
fn pause(duration: Duration, socket: &UnixStream) -> io::Result<()> {
    let end = Instant::now() + duration;
    loop {
        let mut b = 0u8;
        let n = unsafe {
            libc::recv(
                socket.as_raw_fd(),
                (&mut b as *mut u8).cast(),
                1,
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        };
        if STOP.load(Ordering::Relaxed) || n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Gesture cancelled; contact released",
            ));
        }
        if n < 0 && io::Error::last_os_error().kind() != io::ErrorKind::WouldBlock {
            return Err(io::Error::last_os_error());
        }
        let remaining = end.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}
fn native_display() -> Result<Display, String> {
    let mut modes = Vec::new();
    for entry in fs::read_dir("/sys/class/drm").map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if fs::read_to_string(path.join("status"))
            .unwrap_or_default()
            .trim()
            != "connected"
        {
            continue;
        }
        let text = fs::read_to_string(path.join("modes")).map_err(|e| e.to_string())?;
        if let Some((w, h)) = text.lines().next().and_then(|s| s.split_once('x')) {
            modes.push((
                w.parse::<i32>().map_err(|e| e.to_string())?,
                h.parse::<i32>().map_err(|e| e.to_string())?,
            ));
        }
    }
    if modes.len() != 1 {
        return Err(format!(
            "Expected one connected DRM panel, found {}",
            modes.len()
        ));
    }
    let d = Display {
        native_width: modes[0].0,
        native_height: modes[0].1,
        rotation: 0,
    };
    d.validate()?;
    Ok(d)
}
fn peer_allowed(socket: &UnixStream, input_gid: libc::gid_t) -> Option<libc::uid_t> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    } != 0
    {
        return None;
    }
    if cred.uid == 0 {
        return Some(cred.uid);
    }
    // Single-threaded server: libc's static passwd/group storage is not raced.
    let pwd = unsafe { libc::getpwuid(cred.uid) };
    if pwd.is_null() {
        return None;
    }
    let mut groups = vec![0; 256];
    let mut n = groups.len() as i32;
    let rc =
        unsafe { libc::getgrouplist((*pwd).pw_name, (*pwd).pw_gid, groups.as_mut_ptr(), &mut n) };
    (rc >= 0 && groups[..n as usize].contains(&input_gid)).then_some(cred.uid)
}
fn permission_worker(uid: u32) -> Value {
    let result = (|| {
        if unsafe { libc::geteuid() } != 0 {
            return Err(audb_agent::permission::Failure {
                code: audb_protocol::ErrorCode::PermissionDenied,
                message: "Permission worker requires root".into(),
                data: None,
            });
        }
        let bytes = audb_agent::read_frame(std::io::stdin()).map_err(|e| {
            audb_agent::permission::Failure {
                code: audb_protocol::ErrorCode::InvalidArgument,
                message: e.to_string(),
                data: None,
            }
        })?;
        let action: audb_agent::Action =
            serde_json::from_slice(&bytes).map_err(|e| audb_agent::permission::Failure {
                code: audb_protocol::ErrorCode::InvalidArgument,
                message: e.to_string(),
                data: None,
            })?;
        let mut store = audb_agent::permission::Sailjail::connect()?;
        match action {
            audb_agent::Action::PermissionCapabilities => Ok(store.capabilities()),
            audb_agent::Action::Permission {
                application_id,
                action,
            } => audb_agent::permission::execute(&mut store, uid, &application_id, &action),
            _ => Err(audb_agent::permission::Failure {
                code: audb_protocol::ErrorCode::InvalidArgument,
                message: "Invalid permission worker action".into(),
                data: None,
            }),
        }
    })();
    match result {
        Ok(data) => json!({"ok":true,"data":data}),
        Err(e) => e.response(),
    }
}
fn permission_request(uid: u32, action: &audb_agent::Action) -> Value {
    let result = (|| -> Result<Value, String> {
        // Root-owned anonymous file avoids pipe backpressure and user path access.
        let mut output = tempfile::tempfile_in("/run/audb-agent").map_err(|e| e.to_string())?;
        let mut child = Command::new("/usr/sbin/audb-agent")
            .arg("--permission-worker")
            .arg(uid.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::from(output.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let input = serde_json::to_vec(action).map_err(|e| e.to_string())?;
        let written = (|| -> io::Result<()> {
            let mut pipe = child.stdin.take().unwrap();
            pipe.write_all(&input)?;
            pipe.write_all(b"\n")
        })();
        if let Err(e) = written {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e.to_string());
        }
        let begin = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(Some(_)) => return Err("Permission worker failed".into()),
                Ok(None)
                    if begin.elapsed() < Duration::from_secs(20)
                        && !STOP.load(Ordering::Relaxed) =>
                {
                    std::thread::sleep(Duration::from_millis(20))
                }
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(match result {
                        Err(e) => e.to_string(),
                        _ => "Permission worker timed out or service stopped".into(),
                    });
                }
            }
        }
        output.rewind().map_err(|e| e.to_string())?;
        let bytes = audb_agent::read_frame(output).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    })();
    result.unwrap_or_else(|message| json!({"ok":false,"error":{"code":"OUTCOME_UNKNOWN","message":message},"data":{"phase":"permission_worker","mayHaveChanged":true}}))
}
fn handle(
    touch: &mut Option<Touch>,
    keyboard: &mut Option<Keyboard>,
    socket: &UnixStream,
    uid: libc::uid_t,
) -> Value {
    let req: Request = match read_socket_frame(socket, Duration::from_secs(2))
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
    {
        Some(req) => req,
        None => {
            return json!({"ok":false,"error":{"code":"INVALID_ARGUMENT","message":"Invalid agent request"}})
        }
    };
    if req.protocol_version != PROTOCOL {
        return json!({"ok":false,"error":{"code":"PROTOCOL_MISMATCH","message":"Upgrade both audb-agent and audb-agentctl"}});
    }
    if matches!(
        req.action,
        audb_agent::Action::Permission { .. } | audb_agent::Action::PermissionCapabilities
    ) {
        return permission_request(uid, &req.action);
    }
    if let audb_agent::Action::Key { name } = &req.action {
        let Some((code, canonical)) = audb_protocol::input::key_code(name) else {
            return json!({"ok":false,"error":{"code":"INVALID_ARGUMENT","message":"Unsupported key name"}});
        };
        let Some(keyboard) = keyboard.as_mut() else {
            return json!({"ok":false,"error":{"code":"CAPABILITY_UNAVAILABLE","message":"No available uinput keyboard"}});
        };
        return match keyboard.click(code, socket) {
            Ok(()) => {
                json!({"ok":true,"data":{"backend":"uinput","key":canonical,"delivery":"submitted","released":true}})
            }
            Err(e) => {
                json!({"ok":false,"error":{"code":"OUTCOME_UNKNOWN","message":e.to_string()},"data":{"releaseAttempted":true,"replayed":false}})
            }
        };
    }
    if matches!(
        req.action,
        audb_agent::Action::Text { .. } | audb_agent::Action::InputStatus
    ) {
        return json!({"ok":false,"error":{"code":"INVALID_ARGUMENT","message":"Text input uses the unprivileged Maliit bridge through agentctl"}});
    }
    let mut error_code = "INPUT_FAILED";
    let result = (|| -> Result<Value, String> {
        if let Some(display) = req.display {
            display.validate()?;
        }
        if matches!(req.action, audb_agent::Action::Status) {
            let display = req.display.map(|d| { let (w,h) = d.size(); json!({"width":w,"height":h,"nativeWidth":d.native_width,"nativeHeight":d.native_height,"rotation":d.rotation}) });
            return Ok(
                json!({"protocolVersion":PROTOCOL,"version":env!("CARGO_PKG_VERSION"),"backend":"uinput","display":display,
                "capabilities":{"tap":touch.is_some(),"swipe":touch.is_some(),"key":keyboard.is_some(),"screenshot":req.display.is_some(),"permissions":true},"action":req.action}),
            );
        }
        let display = req.display.ok_or("Missing graphical session geometry")?;
        if let Some(touch) = touch.as_ref() {
            if display.native_width != touch.display.native_width
                || display.native_height != touch.display.native_height
            {
                return Err("Graphical session geometry does not match DRM panel".into());
            }
        }
        if let audb_agent::Action::Screenshot { directory } = &req.action {
            error_code = "SCREENSHOT_FAILED";
            audb_agent::screenshot::request(
                uid,
                directory
                    .as_deref()
                    .ok_or("Missing screenshot staging directory")?,
            )?;
        }
        if let Some(g) = gesture(display, &req.action)? {
            let Some(touch) = touch.as_mut() else {
                error_code = "CAPABILITY_UNAVAILABLE";
                return Err("No available uinput touchscreen/DRM panel".into());
            };
            touch.perform(g, socket)?;
            std::thread::sleep(Duration::from_millis(250));
        }
        let (w, h) = display.size();
        Ok(
            json!({"protocolVersion":PROTOCOL,"version":env!("CARGO_PKG_VERSION"),"backend":"uinput",
            "display":{"width":w,"height":h,"nativeWidth":display.native_width,"nativeHeight":display.native_height,"rotation":display.rotation},
            "capabilities":{"tap":touch.is_some(),"swipe":touch.is_some(),"key":keyboard.is_some(),"screenshot":true,"permissions":true},"action":req.action}),
        )
    })();
    match result {
        Ok(data) => json!({"ok":true,"data":data}),
        Err(message) => json!({"ok":false,"error":{"code":error_code,"message":message}}),
    }
}
fn run() -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("audb-agent must run as root".into());
    }
    let group = unsafe { libc::getgrnam(c"input".as_ptr()) };
    if group.is_null() {
        return Err("System input group does not exist".into());
    }
    let gid = unsafe { (*group).gr_gid };
    let mut touch = native_display()
        .and_then(|display| Touch::create(display).map_err(|e| e.to_string()))
        .map_err(|e| eprintln!("Input unavailable; permission service remains available: {e}"))
        .ok();
    let mut keyboard = Keyboard::create()
        .map_err(|e| eprintln!("Keyboard unavailable: {e}"))
        .ok();
    // Allow compositor/udev to discover the persistent device before serving.
    std::thread::sleep(Duration::from_millis(700));
    let dir = std::path::Path::new(SOCKET).parent().unwrap();
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    if let Err(e) = fs::remove_file(SOCKET) {
        if e.kind() != io::ErrorKind::NotFound {
            return Err(e.to_string());
        }
    }
    unsafe {
        libc::umask(0o117);
    }
    let listener = UnixListener::bind(SOCKET).map_err(|e| e.to_string())?;
    if unsafe { libc::chown(c"/run/audb-agent/control.sock".as_ptr(), 0, gid) } != 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    fs::set_permissions(SOCKET, fs::Permissions::from_mode(0o660)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
    }
    eprintln!("audb-agent ready, input available: {}", touch.is_some());
    while !STOP.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut socket, _)) => {
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .map_err(|e| e.to_string())?;
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .map_err(|e| e.to_string())?;
                let Some(uid) = peer_allowed(&socket, gid) else {
                    continue;
                };
                let response = handle(&mut touch, &mut keyboard, &socket, uid);
                let _ = writeln!(socket, "{response}");
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let _ = fs::remove_file(SOCKET);
    Ok(())
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "--permission-worker" {
        if let Ok(uid) = args[1].parse::<u32>() {
            println!("{}", permission_worker(uid));
            return;
        }
    }
    if !args.is_empty() {
        eprintln!("Invalid agent arguments");
        std::process::exit(1);
    }
    if let Err(e) = run() {
        eprintln!("audb-agent: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    #[test]
    fn keyboard_disconnect_releases_pressed_key() {
        let file = tempfile::tempfile().unwrap();
        let mut keyboard = Keyboard {
            file,
            pressed: None,
        };
        let (server, client) = UnixStream::pair().unwrap();
        drop(client);
        assert!(keyboard.click(28, &server).is_err());
        assert!(keyboard.pressed.is_none());
        keyboard.file.rewind().unwrap();
        let mut bytes = Vec::new();
        keyboard.file.read_to_end(&mut bytes).unwrap();
        let size = std::mem::size_of::<InputEvent>();
        let events = bytes
            .chunks_exact(size)
            .map(|chunk| unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast::<InputEvent>()) })
            .collect::<Vec<_>>();
        assert_eq!(
            (events[2].kind, events[2].code, events[2].value),
            (EV_KEY, 28, 0)
        );
        assert_eq!(
            (events[3].kind, events[3].code, events[3].value),
            (EV_SYN, 0, 0)
        );
    }
    #[test]
    fn disconnect_releases_contact_before_next_request() {
        let path = std::env::temp_dir().join(format!("audb-agent-test-{}", std::process::id()));
        let file = File::create(&path).unwrap();
        let mut touch = Touch {
            file,
            active: false,
            tracking: 100,
            display: Display {
                native_width: 1200,
                native_height: 2000,
                rotation: 0,
            },
        };
        let (server, client) = UnixStream::pair().unwrap();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            drop(client);
        });
        let start = Instant::now();
        assert!(touch
            .perform(
                Gesture {
                    start: (10, 20),
                    end: (10, 20),
                    duration: 0,
                    hold: 1000,
                    steps: 0
                },
                &server
            )
            .is_err());
        assert!(!touch.active);
        assert!(start.elapsed() < Duration::from_millis(500));
        t.join().unwrap();
        drop(touch);
        let mut bytes = Vec::new();
        File::open(&path).unwrap().read_to_end(&mut bytes).unwrap();
        fs::remove_file(path).unwrap();
        let size = std::mem::size_of::<InputEvent>();
        let events = bytes
            .chunks_exact(size)
            .map(|chunk| unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast::<InputEvent>()) })
            .collect::<Vec<_>>();
        let last = &events[events.len() - 3..];
        assert_eq!(
            (last[0].kind, last[0].code, last[0].value),
            (EV_KEY, BTN_TOUCH, 0)
        );
        assert_eq!(
            (last[1].kind, last[1].code, last[1].value),
            (EV_ABS, TRACK, -1)
        );
        assert_eq!((last[2].kind, last[2].code, last[2].value), (EV_SYN, 0, 0));
    }
}
