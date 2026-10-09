use audb_protocol::PROTOCOL_VERSION;
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::mpsc,
    time::Duration,
};

struct Workspace {
    dir: tempfile::TempDir,
}
impl Workspace {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_audb"));
        c.env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .stdin(std::process::Stdio::null())
            .args(args);
        let fake_bin = self.dir.path().join("fake-bin");
        if fake_bin.exists() {
            c.env(
                "PATH",
                format!(
                    "{}:{}",
                    fake_bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("AUDB_TEST_LOG", self.dir.path().join("rpm-calls"));
        }
        c
    }
    fn output(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn json(&self, args: &[&str], code: i32) -> Value {
        let output = self.output(args);
        assert_eq!(
            output.status.code(),
            Some(code),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn registry(&self) -> PathBuf {
        self.dir.path().join("config/audb/devices-v1.json")
    }
}

fn fake_missing_agent(w: &Workspace) {
    use std::os::unix::fs::PermissionsExt;
    let bin = w.dir.path().join("fake-bin");
    fs::create_dir_all(&bin).unwrap();
    let script = r#"#!/bin/sh
for arg do
  test "$arg" != -G || { printf 'port 22\nuser defaultuser\n'; exit 0; }
done
cat >/dev/null
printf '%s\n' '{"ok":false,"error":{"code":"AGENT_UNAVAILABLE","message":"setup-device required"}}'
"#;
    fs::write(bin.join("ssh"), script).unwrap();
    fs::set_permissions(bin.join("ssh"), fs::Permissions::from_mode(0o755)).unwrap();
}

/// Exercise CLI -> daemon -> bootstrap transport without touching the host RPM DB.
fn fake_system_installer(w: &Workspace) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = w.dir.path().join("fake-bin");
    fs::create_dir(&bin).unwrap();
    let script = r#"#!/bin/sh
case "$0" in *sftp) cat >/dev/null; exit 0;; esac
for arg do
  test "$arg" != -G || { printf 'port 22\nuser defaultuser\n'; exit 0; }
  command=$arg
done
printf '%s\n' "$command" >> "$AUDB_TEST_LOG"
case "$command" in *devel-su*)
  IFS= read -r credential
  test "$credential" = fixture-credential || { printf 'Auth failed\n' >&2; exit 1; }
  ;;
esac
case "$command" in
  *audb-agentctl*) cat >/dev/null; printf '%s\n' '{"ok":true,"data":{"protocolVersion":1,"backend":"uinput"}}';;
  *mktemp*) printf '/tmp/audb-system-rpm.ABC123\n';;
  *'rpm -qp'*) printf 'audb-agent\t0.3.0\t1\tnoarch\n';;
  *'rpm --eval'*) printf 'aarch64\n';;
  *--test*)
    test -z "${AUDB_TEST_DELAY:-}" || sleep "$AUDB_TEST_DELAY"
    test "${AUDB_TEST_FAIL_CHECK:-}" != 1 || { printf 'dependencies failed\n' >&2; exit 1; }
    printf 'fixture-credential\n';;
  *--undefine*) printf 'installed\n';;
  *'rpm -q '*) printf 'audb-agent\t0.3.0\t1\tnoarch\n';;
esac
"#;
    for name in ["ssh", "sftp"] {
        let path = bin.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let rpm = w.dir.path().join("fixture's agent.rpm");
    fs::write(&rpm, [0xed, 0xab, 0xee, 0xdb]).unwrap();
    w.json(
        &[
            "--json",
            "device",
            "add",
            "defaultuser@127.0.0.1",
            "--id",
            "phone",
        ],
        0,
    );
    rpm
}

fn credential_command(w: &Workspace, args: &[&str]) -> Output {
    let mut child = w
        .command(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"fixture-credential\n")
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn system_installer_bootstraps_via_stdin_and_setup_checks_our_package() {
    let w = Workspace::new();
    let rpm = fake_system_installer(&w);
    let result = credential_command(
        &w,
        &[
            "--json",
            "--device",
            "phone",
            "setup-device",
            "--rpm",
            rpm.to_str().unwrap(),
            "--root-password-stdin",
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(!output.contains("fixture-credential"));
    let value: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(value["deviceId"], "phone");
    assert_eq!(value["data"]["package"]["name"], "audb-agent");
    assert_eq!(value["data"]["installed"], true);
    assert_eq!(value["data"]["stagingCleanup"], true);
    let log = fs::read_to_string(w.dir.path().join("rpm-calls")).unwrap();
    assert!(!log.contains("fixture-credential"));
    assert_eq!(
        log.lines()
            .filter(|c| c.contains("devel-su") && c.contains("--undefine") && !c.contains("--test"))
            .count(),
        1
    );
    assert!(log.contains("rpm -ivh --test --undefine=__transaction_validation"));
    assert!(!fs::read_to_string(w.registry())
        .unwrap()
        .contains("fixture-credential"));
}

#[test]
fn system_installer_check_only_and_missing_credentials_are_explicit() {
    let w = Workspace::new();
    let rpm = fake_system_installer(&w);
    let error = w.json(
        &[
            "--json",
            "--device",
            "phone",
            "package",
            "install-system",
            rpm.to_str().unwrap(),
        ],
        1,
    );
    assert_eq!(error["error"]["code"], "ROOT_ACCESS_REQUIRED");
    let result = credential_command(
        &w,
        &[
            "--json",
            "--device",
            "phone",
            "package",
            "install-system",
            rpm.to_str().unwrap(),
            "--check-only",
            "--upgrade",
            "--reinstall",
            "--root-password-stdin",
        ],
    );
    assert!(result.status.success());
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["data"]["installed"], false);
    assert_eq!(value["data"]["checkOnly"], true);
    let log = fs::read_to_string(w.dir.path().join("rpm-calls")).unwrap();
    assert!(log.contains("rpm -Uvh --test --replacepkgs"));
    assert_eq!(
        log.lines()
            .filter(|c| c.contains("--undefine") && !c.contains("--test"))
            .count(),
        0
    );
    let missing = w.json(
        &[
            "--json",
            "--device",
            "phone",
            "setup-device",
            "--rpm",
            "missing.rpm",
        ],
        4,
    );
    assert_eq!(missing["error"]["code"], "NOT_FOUND");
}

#[test]
fn deadline_cleans_staging_without_dispatching_install() {
    let w = Workspace::new();
    let rpm = fake_system_installer(&w);
    let mut child = w
        .command(&[
            "--json",
            "--device",
            "phone",
            "--command-timeout",
            "1",
            "package",
            "install-system",
            rpm.to_str().unwrap(),
            "--root-password-stdin",
        ])
        .env("AUDB_TEST_DELAY", "2")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"fixture-credential\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "OUTCOME_UNKNOWN");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let log = fs::read_to_string(w.dir.path().join("rpm-calls")).unwrap();
        assert_eq!(
            log.lines()
                .filter(|c| c.contains("--undefine") && !c.contains("--test"))
                .count(),
            0
        );
        if log.contains("rmdir") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Cancellation left staging without a cleanup attempt"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn authentication_failure_is_reported_without_installing_or_leaking_input() {
    let w = Workspace::new();
    let rpm = fake_system_installer(&w);
    let mut child = w
        .command(&[
            "--json",
            "--device",
            "phone",
            "package",
            "install-system",
            rpm.to_str().unwrap(),
            "--root-password-stdin",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"incorrect-fixture-credential\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("incorrect-fixture-credential"));
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["error"]["code"], "AUTHENTICATION_FAILED");
    assert_eq!(value["data"]["phase"], "check");
    assert_eq!(value["data"]["stagingCleanup"], true);
    let log = fs::read_to_string(w.dir.path().join("rpm-calls")).unwrap();
    assert_eq!(
        log.lines()
            .filter(|c| c.contains("--undefine") && !c.contains("--test"))
            .count(),
        0
    );
}
impl Drop for Workspace {
    fn drop(&mut self) {
        if self
            .dir
            .path()
            .join(format!("cache/audb/audb-v{PROTOCOL_VERSION}.sock"))
            .exists()
        {
            let _ = self.output(&["__shutdown"]);
            std::thread::sleep(Duration::from_millis(40));
        }
    }
}

#[test]
fn registry_selection_and_errors_have_actual_device_id() {
    let w = Workspace::new();
    fake_missing_agent(&w);
    let d = w.json(
        &[
            "--json",
            "device",
            "add",
            "defaultuser@127.0.0.1",
            "--id",
            "phone",
        ],
        0,
    );
    assert_eq!(d["deviceId"], "phone");
    assert_eq!(d["data"]["kind"], "physical");
    assert!(d["data"].get("emulator").is_none());
    let list = w.json(&["--json", "device", "list"], 0);
    assert!(list["deviceId"].is_null());
    assert_eq!(list["data"].as_array().unwrap().len(), 2);
    w.json(&["--json", "select", "phone"], 0);
    // Never connect this test to an emulator running on the developer's host.
    let absent_qmp = w.dir.path().join("absent-qmp.sock");
    w.json(
        &[
            "--json",
            "device",
            "update",
            "emulator",
            "--qmp",
            absent_qmp.to_str().unwrap(),
        ],
        0,
    );
    let error = w.json(&["--json", "--device", "emulator", "status"], 3);
    assert_eq!(error["deviceId"], "emulator");
    assert_eq!(
        w.json(&["--json", "device", "current"], 0)["data"]["id"],
        "phone"
    );
    let error = w.json(&["--json", "tap", "10", "10"], 10);
    assert_eq!(error["deviceId"], "phone");
    assert_eq!(error["error"]["code"], "CAPABILITY_UNAVAILABLE");
    let error = w.json(&["--json", "--device", "missing", "status"], 1);
    assert!(error["deviceId"].is_null());
    assert_eq!(error["error"]["code"], "DEVICE_NOT_FOUND");
    w.json(&["--json", "device", "remove", "phone"], 0);
    assert_eq!(
        w.json(&["--json", "device", "current"], 0)["data"]["id"],
        "emulator"
    );
    w.json(&["--json", "device", "remove", "emulator"], 0);
    assert_eq!(
        w.json(&["--json", "status"], 1)["error"]["code"],
        "DEVICE_REQUIRED"
    );
    let before = fs::read(w.registry()).unwrap();
    w.json(
        &[
            "--json",
            "device",
            "add",
            "--host",
            "127.0.0.1",
            "--id",
            "bad",
            "--kind",
            "physical",
            "--qmp",
            "/tmp/no.sock",
        ],
        1,
    );
    assert_eq!(fs::read(w.registry()).unwrap(), before);
}

/// Fake QMP endpoint validates actual request routing without running a VM.
fn qmp_server(path: &Path, label: &'static str, delay: Duration) -> mpsc::Receiver<()> {
    let listener = UnixListener::bind(path).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut io = BufReader::new(stream);
        io.get_mut().write_all(b"{\"QMP\":{}}\n").unwrap();
        loop {
            let mut line = String::new();
            if !matches!(io.read_line(&mut line), Ok(n) if n > 0) {
                break;
            }
            let request: Value = serde_json::from_str(&line).unwrap();
            let result = if request["execute"] == "query-status" {
                let _ = tx.send(());
                std::thread::sleep(delay);
                json!({"status": label})
            } else if request["execute"] == "query-commands" {
                json!([])
            } else {
                json!({})
            };
            if io
                .get_mut()
                .write_all(format!("{}\n", json!({"return":result,"id":request["id"]})).as_bytes())
                .is_err()
            {
                break;
            }
        }
    });
    rx
}

#[test]
fn daemon_routes_independently_and_reloads_updated_configuration() {
    let w = Workspace::new();
    fake_missing_agent(&w);
    let first = w.dir.path().join("first.sock");
    let second = w.dir.path().join("second.sock");
    let key = w.dir.path().join("key");
    fs::write(&key, "placeholder").unwrap();
    let ready = qmp_server(&first, "first", Duration::from_millis(1500));
    let _second_ready = qmp_server(&second, "second", Duration::ZERO);
    w.json(
        &[
            "--json",
            "device",
            "add",
            "--id",
            "vm",
            "--host",
            "127.0.0.1",
            "--kind",
            "emulator",
            "--key",
            key.to_str().unwrap(),
            "--qmp",
            first.to_str().unwrap(),
        ],
        0,
    );
    w.json(
        &[
            "--json",
            "device",
            "add",
            "defaultuser@127.0.0.1",
            "--id",
            "phone",
        ],
        0,
    );
    // The second command begins only after the first is executing inside QMP.
    let slow = w
        .command(&["--json", "--device", "vm", "status"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let fast = w.json(
        &[
            "--json",
            "--command-timeout",
            "1",
            "--device",
            "phone",
            "tap",
            "10",
            "10",
        ],
        10,
    );
    assert_eq!(fast["error"]["code"], "CAPABILITY_UNAVAILABLE");
    let output = slow.wait_with_output().unwrap();
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["data"]["status"]["status"], "first");
    w.json(
        &[
            "--json",
            "device",
            "update",
            "vm",
            "--qmp",
            second.to_str().unwrap(),
        ],
        0,
    );
    let result = w.json(&["--json", "--device", "vm", "status"], 0);
    assert_eq!(result["data"]["status"]["status"], "second");
    w.json(&["--json", "device", "remove", "vm"], 0);
    assert_eq!(
        w.json(&["--json", "--device", "vm", "status"], 1)["error"]["code"],
        "DEVICE_NOT_FOUND"
    );
}

#[test]
fn older_registry_is_preserved_and_invalid_versions_are_rejected() {
    let w = Workspace::new();
    let parent = w.registry().parent().unwrap().to_owned();
    fs::create_dir_all(&parent).unwrap();
    let old = parent.join("devices.json");
    let bytes = b"{\"schema-version\":0,\"devices\":[]}";
    fs::write(&old, bytes).unwrap();
    w.json(&["--json", "device", "list"], 0);
    assert_eq!(fs::read(&old).unwrap(), bytes);
    let mut registry: Value = serde_json::from_slice(&fs::read(w.registry()).unwrap()).unwrap();
    registry["version"] = json!(999);
    let bytes = serde_json::to_vec(&registry).unwrap();
    fs::write(w.registry(), &bytes).unwrap();
    assert_eq!(
        w.json(&["--json", "device", "list"], 1)["error"]["code"],
        "PROTOCOL_MISMATCH"
    );
    assert_eq!(fs::read(w.registry()).unwrap(), bytes);
}

#[test]
fn deadline_returns_without_repeating_a_qmp_command() {
    let w = Workspace::new();
    let socket = w.dir.path().join("delayed.sock");
    let ready = qmp_server(&socket, "late", Duration::from_millis(1500));
    w.json(
        &[
            "--json",
            "device",
            "update",
            "emulator",
            "--qmp",
            socket.to_str().unwrap(),
        ],
        0,
    );
    let child = w
        .command(&["--json", "--command-timeout", "1", "status"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["deviceId"], "emulator");
    assert_eq!(error["error"]["code"], "OUTCOME_UNKNOWN");
    // The test server signals each query-status: a timeout must not replay it.
    assert!(ready.recv_timeout(Duration::from_millis(650)).is_err());
}

#[test]
fn concurrent_first_requests_share_one_daemon() {
    let w = Workspace::new();
    fake_missing_agent(&w);
    w.json(
        &[
            "--json",
            "device",
            "add",
            "defaultuser@127.0.0.1",
            "--id",
            "phone",
        ],
        0,
    );
    let first = w
        .command(&["--json", "--device", "phone", "tap", "1", "1"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let second = w
        .command(&["--json", "--device", "phone", "tap", "2", "2"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    for child in [first, second] {
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(10));
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["error"]["code"], "CAPABILITY_UNAVAILABLE");
    }
    assert_eq!(
        w.json(&["--json", "--device", "phone", "tap", "3", "3"], 10)["deviceId"],
        "phone"
    );
}

#[test]
fn physical_input_uses_stdin_preserves_options_and_reports_validation() {
    use std::os::unix::fs::PermissionsExt;
    let w = Workspace::new();
    let bin = w.dir.path().join("fake-bin");
    fs::create_dir(&bin).unwrap();
    let script = r#"#!/usr/bin/python3
import json,sys
if '-G' in sys.argv:
    print('port 22\nuser defaultuser')
else:
    action=json.load(sys.stdin)
    assert '"command":"tap"' not in sys.argv[-1]
    assert '"command":"swipe"' not in sys.argv[-1]
    if action.get('x',0)<0:
        print(json.dumps({'ok':False,'error':{'code':'INVALID_ARGUMENT','message':'outside screen'}}))
    else:
        print(json.dumps({'ok':True,'data':{'protocolVersion':1,'backend':'uinput','action':action}}))
"#;
    fs::write(bin.join("ssh"), script).unwrap();
    fs::set_permissions(bin.join("ssh"), fs::Permissions::from_mode(0o755)).unwrap();
    w.json(
        &[
            "--json",
            "device",
            "add",
            "defaultuser@127.0.0.1",
            "--id",
            "phone",
        ],
        0,
    );
    let value = w.json(
        &[
            "--json",
            "--device",
            "phone",
            "tap",
            "940",
            "323",
            "--duration",
            "80",
        ],
        0,
    );
    assert_eq!(value["data"]["action"]["x"], 940);
    assert_eq!(value["data"]["action"]["duration_ms"], 80);
    let value = w.json(
        &[
            "--json",
            "--device",
            "phone",
            "swipe",
            "up",
            "--duration",
            "800",
            "--hold",
            "40",
            "--steps",
            "60",
        ],
        0,
    );
    assert_eq!(value["data"]["action"]["args"], json!(["up"]));
    assert_eq!(value["data"]["action"]["duration_ms"], 800);
    assert_eq!(value["data"]["action"]["hold_ms"], 40);
    assert_eq!(value["data"]["action"]["steps"], 60);
    let text = "Привет $\"\\ 🦀\nsecond line\n";
    let mut child = w
        .command(&[
            "--json", "--device", "phone", "text", "--stdin", "--delay", "0",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"]["action"]["text"], text);
    assert_eq!(
        w.json(&["--json", "--device", "phone", "key", "backspace"], 0)["data"]["action"]["name"],
        "backspace"
    );
    assert_eq!(
        w.json(
            &["--json", "--device", "phone", "text", "bad\u{1}", "--delay", "0"],
            1
        )["error"]["code"],
        "INVALID_ARGUMENT"
    );
    let value = w.json(
        &[
            "--json",
            "--device",
            "phone",
            "--socket",
            "/tmp/wrong-qmp",
            "tap",
            "1",
            "1",
        ],
        1,
    );
    assert_eq!(value["error"]["code"], "INVALID_ARGUMENT");
}

#[test]
fn physical_screenshot_preserves_binary_stdout_and_json_file_metadata() {
    use std::os::unix::fs::PermissionsExt;
    const PNG: &[u8] = include_bytes!("../../audb-agent/tests/one-pixel.png");
    let w = Workspace::new();
    let bin = w.dir.path().join("fake-bin");
    fs::create_dir(&bin).unwrap();
    fs::write(w.dir.path().join("rpm-calls.png"), PNG).unwrap();
    let script = r#"#!/usr/bin/python3
import json,sys,os
from pathlib import Path
if '-G' in sys.argv:
    print('port 22\nuser defaultuser')
    sys.exit(0)
a=json.load(sys.stdin)
p=Path(os.environ['AUDB_TEST_LOG'])
if a['command']=='status':
    print(json.dumps({'ok':True,'data':{'protocolVersion':1,'capabilities':{'screenshot':True}}}))
elif p.with_suffix('.fail').exists():
    print(json.dumps({'ok':False,'error':{'code':'SCREENSHOT_FAILED','message':'capture failed'}}))
else:
    png=p.with_suffix('.png').read_bytes()
    header=json.dumps({'ok':True,'data':{'format':'png','bytes':len(png),'width':1,'height':1}}).encode()+b'\n'
    if p.with_suffix('.truncate').exists(): png=png[:-1]
    sys.stdout.buffer.write(header+png)
"#;
    fs::write(bin.join("ssh"), script).unwrap();
    fs::set_permissions(bin.join("ssh"), fs::Permissions::from_mode(0o755)).unwrap();
    w.json(
        &[
            "--json",
            "device",
            "add",
            "defaultuser@127.0.0.1",
            "--id",
            "phone",
        ],
        0,
    );
    let path = w.dir.path().join("screen with spaces.png");
    let result = w.json(
        &[
            "--device",
            "phone",
            "--json",
            "screenshot",
            "--output",
            path.to_str().unwrap(),
        ],
        0,
    );
    assert_eq!(result["data"]["format"], "png");
    assert_eq!(result["data"]["width"], 1);
    assert_eq!(result["data"]["height"], 1);
    assert_eq!(fs::read(&path).unwrap(), PNG);
    let output = w.output(&["--device", "phone", "screenshot"]);
    assert!(output.status.success());
    assert_eq!(output.stdout, PNG);
    let result = w.json(&["--device", "phone", "--json", "screenshot"], 1);
    assert_eq!(result["error"]["code"], "INVALID_ARGUMENT");
    fs::write(w.dir.path().join("rpm-calls.truncate"), b"").unwrap();
    let result = w.json(
        &[
            "--device",
            "phone",
            "--json",
            "screenshot",
            "--output",
            path.to_str().unwrap(),
        ],
        1,
    );
    assert_eq!(result["error"]["code"], "RUNTIME_ERROR");
    assert_eq!(fs::read(&path).unwrap(), PNG);
    fs::write(w.dir.path().join("rpm-calls.fail"), b"").unwrap();
    let result = w.json(
        &[
            "--device",
            "phone",
            "--json",
            "screenshot",
            "--output",
            path.to_str().unwrap(),
        ],
        1,
    );
    assert_eq!(result["error"]["message"], "capture failed");
    assert_eq!(fs::read(&path).unwrap(), PNG);
}

#[test]
fn emulator_screenshot_still_uses_qmp() {
    const PNG: &[u8] = include_bytes!("../../audb-agent/tests/one-pixel.png");
    let w = Workspace::new();
    let socket = w.dir.path().join("qmp.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut io = BufReader::new(stream);
        io.get_mut().write_all(b"{\"QMP\":{}}\n").unwrap();
        for _ in 0..2 {
            let mut line = String::new();
            io.read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            if req["execute"] == "screendump" {
                fs::write(req["arguments"]["filename"].as_str().unwrap(), PNG).unwrap();
            }
            io.get_mut()
                .write_all(format!("{}\n", json!({"return":{},"id":req["id"]})).as_bytes())
                .unwrap();
        }
    });
    let output = w.output(&["--socket", socket.to_str().unwrap(), "screenshot"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, PNG);
    server.join().unwrap();
}

#[test]
fn permission_cli_routes_structured_requests_and_preserves_partial_errors() {
    use std::os::unix::fs::PermissionsExt;
    let w = Workspace::new();
    let bin = w.dir.path().join("fake-bin");
    fs::create_dir(&bin).unwrap();
    let script = r#"#!/usr/bin/python3
import sys,json,os
if '-G' in sys.argv:
    print('port 22\nuser defaultuser\n');sys.exit()
x=json.load(sys.stdin)
with open(os.environ['AUDB_TEST_LOG'],'a') as f:f.write(json.dumps(x)+'\n')
if x['command']=='permission_capabilities':
    print(json.dumps({'ok':True,'data':{'permissions':True,'promptSemantics':'aurora-boolean'}}))
else:
    a=x['action']
    if x['application_id']=='missing':
        print(json.dumps({'ok':False,'error':{'code':'APP_NOT_FOUND','message':'unknown'}}))
    elif a['operation']=='reset':
        print(json.dumps({'ok':False,'error':{'code':'PERMISSION_VERIFY_FAILED','message':'partial'},'data':{'phase':'verify','before':{'granted':['UserDirs']},'after':{'granted':[]}}}))
    else:
        print(json.dumps({'ok':True,'data':{'uid':100123,'applicationId':x['application_id'],'action':a}}))
"#;
    fs::write(bin.join("ssh"), script).unwrap();
    fs::set_permissions(bin.join("ssh"), fs::Permissions::from_mode(0o755)).unwrap();
    w.json(
        &[
            "--json",
            "device",
            "add",
            "defaultuser@127.0.0.1",
            "--id",
            "phone",
        ],
        0,
    );
    let grant = w.json(
        &[
            "--json",
            "--device",
            "phone",
            "permission",
            "grant",
            "test-app",
            "UserDirs",
            "DeviceInfo",
            "--disable-prompt",
        ],
        0,
    );
    assert_eq!(grant["deviceId"], "phone");
    assert_eq!(
        grant["data"]["action"]["permissions"],
        json!(["UserDirs", "DeviceInfo"])
    );
    assert_eq!(grant["data"]["action"]["disablePrompt"], true);
    let all = w.json(
        &[
            "--json",
            "--device",
            "phone",
            "permission",
            "grant",
            "test-app",
            "--all-requested",
            "--disable-prompt",
        ],
        0,
    );
    assert_eq!(all["data"]["action"]["allRequested"], true);
    let prompt = w.json(
        &[
            "--json",
            "--device",
            "phone",
            "permission",
            "prompt",
            "test-app",
            "--disable",
        ],
        0,
    );
    assert_eq!(prompt["data"]["action"]["enabled"], false);
    assert_eq!(
        w.json(
            &[
                "--json",
                "--device",
                "phone",
                "permission",
                "list",
                "missing"
            ],
            1
        )["error"]["code"],
        "APP_NOT_FOUND"
    );
    let partial = w.json(
        &[
            "--json",
            "--device",
            "phone",
            "permission",
            "reset",
            "test-app",
        ],
        1,
    );
    assert_eq!(partial["error"]["code"], "PERMISSION_VERIFY_FAILED");
    assert_eq!(partial["data"]["phase"], "verify");
    assert_eq!(partial["data"]["after"]["granted"], json!([]));
    for args in [
        vec!["permission", "grant", "test-app"],
        vec![
            "permission",
            "grant",
            "test-app",
            "UserDirs",
            "--all-requested",
        ],
        vec!["permission", "prompt", "test-app"],
        vec!["permission", "prompt", "test-app", "--enable", "--disable"],
    ] {
        assert!(!w.output(&args).status.success());
    }
    let records = fs::read_to_string(w.dir.path().join("rpm-calls")).unwrap();
    assert!(!records.contains("display"));
    assert!(!records.contains("devel-su"));
}

fn fake_readiness(w: &Workspace, scenario: &str) {
    use std::os::unix::fs::PermissionsExt;
    let bin = w.dir.path().join("fake-bin");
    fs::create_dir(&bin).unwrap();
    let script = format!("#!/usr/bin/python3\nscenario = {scenario:?}\n")
        + r#"
import json, os, shlex, sys
if '-G' in sys.argv:
    print('port 22\nuser defaultuser')
    sys.exit(0)
if os.path.basename(sys.argv[0]) == 'sftp':
    command = shlex.split(sys.stdin.read())
    assert command[0:2] == ['get', '/etc/os-release'], command
    with open(command[2], 'w') as f:
        f.write('ID=aurora\nVERSION_ID="5.2.test"\n')
    sys.exit(0)
command = sys.argv[-1]
with open(os.environ['AUDB_TEST_LOG'], 'a') as f:
    f.write(json.dumps({'shell': command}) + '\n')
if command.startswith('# audb doctor inventory v1'):
    if scenario == 'offline':
        print('fixture device offline', file=sys.stderr)
        sys.exit(255)
    print('uid\t100000\narchitecture\taarch64\nsession\ttrue\ntool.gdbus\ttrue\ntool.rpm\ttrue\ntool.journalctl\ttrue\nagentRpm\t0.3.0-10')
elif 'audb-agentctl' in command:
    action = json.load(sys.stdin)
    assert action['command'] in ['status', 'permission_capabilities'], action
    with open(os.environ['AUDB_TEST_LOG'], 'a') as f:
        f.write(json.dumps({'agent':action}) + '\n')
    if scenario == 'missing':
        print(json.dumps({'ok':False,'error':{'code':'AGENT_UNAVAILABLE','message':'setup-device required'}}))
    elif action['command'] == 'status':
        print(json.dumps({'ok':True,'data':{'protocolVersion':2 if scenario == 'old' else 3,
            'version':'0.3.0','display':{'width':2000,'height':1200},
            'capabilities':dict.fromkeys(['tap','swipe','key','text','screenshot'],True),'input':{'active':False}}}))
    else:
        print(json.dumps({'ok':True,'data':{'permissions':True,'promptSemantics':'aurora-boolean'}}))
elif 'com.nokia.mce.request.get_' in command:
    method = command.rsplit('.',1)[-1]
    states = {'get_display_status':'on','get_tklock_mode':'unlocked','get_touch_input_policy':'enabled','get_display_blanking_inhibit':'disabled'}
    print(repr((states[method],)))
elif command.endswith('Control1.GetRunningApplications'):
    print('([], [])')
elif command.endswith('ru.omp.APM.GetPackageList'):
    print("(['ru.test.Probe'],)")
else:
    raise AssertionError('Unexpected or modifying command: ' + command)
"#;
    for name in ["ssh", "sftp"] {
        let path = bin.join(name);
        fs::write(&path, &script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    w.json(
        &[
            "--json",
            "device",
            "add",
            "defaultuser@127.0.0.1",
            "--id",
            "phone",
        ],
        0,
    );
}

#[test]
fn doctor_and_capabilities_report_readiness_without_mutating_device() {
    let w = Workspace::new();
    fake_readiness(&w, "ready");
    let report = w.json(&["--device", "phone", "--json", "doctor"], 0);
    assert_eq!(report["schemaVersion"], 1);
    assert_eq!(report["deviceId"], "phone");
    assert_eq!(report["data"]["reportVersion"], 1);
    assert_eq!(report["data"]["healthy"], true);
    let checks = report["data"]["checks"].as_array().unwrap();
    assert!(checks
        .iter()
        .any(|c| c["name"] == "identity" && c["data"]["osVersion"] == "5.2.test"));
    assert!(checks
        .iter()
        .any(|c| c["name"] == "inputFocus" && c["status"] == "warning"));
    let caps = &report["data"]["capabilities"];
    assert_eq!(caps["text"]["available"], true);
    assert_eq!(caps["text"]["ready"], false);
    assert_eq!(caps["text"]["reasonCode"], "INPUT_NOT_FOCUSED");
    assert_eq!(caps["tap"]["ready"], true);
    assert_eq!(caps["permissions"]["ready"], true);
    assert_eq!(caps["files"]["ready"], true);
    assert_eq!(caps["logs"]["ready"], false);
    assert_eq!(caps["displayStatus"]["ready"], true);
    assert_eq!(caps["displayControl"]["ready"], false);
    let report = w.json(&["--device", "phone", "--json", "capabilities"], 0);
    assert_eq!(report["data"]["capabilities"], *caps);
    assert!(report["data"].get("checks").is_none());
}

#[test]
fn doctor_preserves_missing_agent_and_protocol_errors_with_fixes() {
    for (scenario, code) in [
        ("missing", "CAPABILITY_UNAVAILABLE"),
        ("old", "PROTOCOL_MISMATCH"),
    ] {
        let w = Workspace::new();
        fake_readiness(&w, scenario);
        let report = w.json(&["--device", "phone", "--json", "doctor"], 0);
        assert_eq!(report["ok"], true);
        assert_eq!(report["data"]["healthy"], false);
        let check = report["data"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "agent")
            .unwrap();
        assert_eq!(check["code"], code);
        assert!(check["fix"].as_str().unwrap().contains("setup-device"));
        assert_eq!(report["data"]["capabilities"]["text"]["ready"], false);
        assert_eq!(report["data"]["capabilities"]["shell"]["ready"], true);
        let log = fs::read_to_string(w.dir.path().join("rpm-calls")).unwrap();
        assert!(!log.contains("permission_capabilities"));
    }
}

#[test]
fn offline_device_returns_report_and_skips_dependent_probes() {
    let w = Workspace::new();
    fake_readiness(&w, "offline");
    let report = w.json(&["--device", "phone", "--json", "doctor"], 0);
    assert_eq!(report["data"]["healthy"], false);
    let checks = report["data"]["checks"].as_array().unwrap();
    assert!(checks
        .iter()
        .any(|c| c["name"] == "ssh" && c["status"] == "failed"));
    assert!(checks
        .iter()
        .any(|c| c["name"] == "agent" && c["status"] == "skipped"));
    assert_eq!(report["data"]["capabilities"]["shell"]["ready"], false);
    assert_eq!(
        fs::read_to_string(w.dir.path().join("rpm-calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

fn fake_app_installer(w: &Workspace) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = w.dir.path().join("fake-bin");
    fs::create_dir(&bin).unwrap();
    let script = r#"#!/usr/bin/python3
import hashlib,json,os,pathlib,secrets,shlex,shutil,sys
log=pathlib.Path(os.environ['AUDB_TEST_LOG']); db=log.with_suffix('.app-state')
state=json.loads(db.read_text()) if db.exists() else {'installed':False,'polls':0}
def save():db.write_text(json.dumps(state))
def event(v):
    with log.open('a') as f:f.write(json.dumps(v)+'\n')
if '-G' in sys.argv:
    print('port 22\nuser defaultuser');sys.exit(0)
if pathlib.Path(sys.argv[0]).name=='sftp':
    args=shlex.split(sys.stdin.read());assert args[0]=='put'
    shutil.copyfile(args[1],args[2]);event({'uploadBytes':os.path.getsize(args[2]),'path':args[2]});sys.exit(0)
cmd=sys.argv[-1];event({'shell':cmd})
if 'mktemp -d /tmp/audb-app-rpm.' in cmd:
    stage='/tmp/audb-app-rpm.'+secrets.token_hex(3);os.mkdir(stage,0o700);state['stage']=stage;save();print(stage)
elif cmd.startswith('rpm -qp '):
    print('ru.test.Probe\t0.1.0+1\t1\tx86_64')
elif cmd.startswith('rpm --eval '):print('x86_64')
elif 'ru.omp.APM.Install ' in cmd:
    state['installed']=True;state['polls']=0;save()
    if log.with_suffix('.lost').exists():
        print('fixture connection lost after dispatch',file=sys.stderr);sys.exit(255)
    print('()')
elif 'ru.omp.APM.GetPackage ' in cmd:
    if state['installed']:
        state['polls']+=1;save()
    if not state['installed'] or state['polls']<3:
        print('Error: GDBus.Error:ru.omp.APM.Error.PackageNotExist: The package does not exist',file=sys.stderr);sys.exit(1)
    print("({'general.id': 'ru.test.Probe', 'general.version': '0.1.0+1-1'},)")
elif cmd.startswith('rm -f -- '):
    args=shlex.split(cmd);file=args[3];stage=args[-1]
    assert stage==state['stage'] and file==stage+'/package.rpm'
    pathlib.Path(file).unlink(missing_ok=True);os.rmdir(stage);event({'cleaned':True})
else:raise AssertionError('Unexpected command: '+cmd)
"#;
    for name in ["ssh", "sftp"] {
        let path = bin.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    w.json(
        &[
            "--json",
            "device",
            "add",
            "defaultuser@127.0.0.1",
            "--id",
            "phone",
        ],
        0,
    );
    let path = w.dir.path().join("large 'quoted $package.rpm");
    let mut file = fs::File::create(&path).unwrap();
    file.write_all(&[0xed, 0xab, 0xee, 0xdb]).unwrap();
    let block = vec![255u8; 1024 * 1024];
    for _ in 0..34 {
        file.write_all(&block).unwrap();
    }
    path
}

#[test]
fn large_rpm_uses_sftp_and_waits_for_registration_without_reinstalling() {
    let w = Workspace::new();
    let rpm = fake_app_installer(&w);
    let args = [
        "--device",
        "phone",
        "--json",
        "package",
        "install",
        rpm.to_str().unwrap(),
    ];
    let result = w.json(&args, 0);
    let data = &result["data"];
    assert_eq!(data["bytes"], 34 * 1024 * 1024 + 4);
    assert_eq!(data["verified"], true);
    assert_eq!(data["installed"], true);
    assert_eq!(data["alreadyInstalled"], false);
    assert_eq!(data["stagingCleanup"], true);
    assert_eq!(data["rpm"]["version"], "0.1.0+1");
    let result = w.json(&args, 0);
    assert_eq!(result["data"]["alreadyInstalled"], true);
    assert_eq!(result["data"]["changed"], false);
    let log = fs::read_to_string(w.dir.path().join("rpm-calls")).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("ru.omp.APM.Install "))
            .count(),
        1
    );
    let entries: Vec<Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(entries
        .iter()
        .any(|v| v["uploadBytes"] == 34 * 1024 * 1024 + 4));
    assert_eq!(entries.iter().filter(|v| v["cleaned"] == true).count(), 2);
    assert!(
        entries
            .iter()
            .filter(|v| v["shell"]
                .as_str()
                .is_some_and(|s| s.contains("ru.omp.APM.GetPackage ")))
            .count()
            >= 5
    );
}

#[test]
fn lost_install_response_is_not_replayed_or_removed_under_apm() {
    let w = Workspace::new();
    let rpm = fake_app_installer(&w);
    fs::write(w.dir.path().join("rpm-calls.lost"), b"").unwrap();
    let result = w.json(
        &[
            "--device",
            "phone",
            "--json",
            "package",
            "install",
            rpm.to_str().unwrap(),
        ],
        1,
    );
    assert_eq!(result["error"]["code"], "OUTCOME_UNKNOWN");
    assert_eq!(result["data"]["stagingRetained"], true);
    let log = fs::read_to_string(w.dir.path().join("rpm-calls")).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("ru.omp.APM.Install "))
            .count(),
        1
    );
    assert!(!log.contains("cleaned"));
    // Remove this test's retained files after verifying retention semantics.
    let stage = result["data"]["stagingDirectory"].as_str().unwrap();
    fs::remove_file(Path::new(stage).join("package.rpm")).unwrap();
    fs::remove_dir(stage).unwrap();
}
