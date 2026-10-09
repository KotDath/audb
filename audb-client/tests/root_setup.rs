#![cfg(unix)]
use serde_json::Value;
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
};

struct Fixture(tempfile::TempDir);
impl Fixture {
    fn new() -> Self {
        let fixture = Self(tempfile::tempdir().unwrap());
        let bin = fixture.0.path().join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(
            bin.join("ssh"),
            r#"#!/usr/bin/python3
import sys, os, json
from pathlib import Path
base=Path(os.environ['AUDB_ROOT_FIXTURE'])
args=sys.argv[1:]
if '-G' in args:
 print('user defaultuser\nport 22');sys.exit(0)
command=args[-1]
with (base/'calls').open('a') as f: f.write(json.dumps(args)+'\n')
if 'devel-su' in command:
 password=sys.stdin.buffer.readline().decode().rstrip('\n')
 if password!='fixture-password' or (base/'bad-password').exists():
  print('Auth failed: '+password,file=sys.stderr);sys.exit(1)
 if 'AUDB_KEY_ADDED' in command:
  (base/'root-key').touch();print('AUDB_KEY_ADDED')
 elif 'grep -Fvx' in command:
  (base/'root-key').unlink(missing_ok=True)
 sys.exit(0)
if 'User=root' in args:
 if not (base/'root-key').exists() or (base/'deny-root').exists(): sys.exit(255)
 print('0');sys.exit(0)
if 'AUDB_KEY_ADDED' in command:
 (base/'user-key').touch();print('AUDB_KEY_ADDED')
elif 'grep -Fvx' in command:
 (base/'user-key').unlink(missing_ok=True)
elif command=='id -u': print('100000')
"#,
        )
        .unwrap();
        fs::set_permissions(bin.join("ssh"), fs::Permissions::from_mode(0o755)).unwrap();
        let result = fixture.run(
            &["device", "add", "defaultuser@127.0.0.1", "--id", "phone"],
            false,
        );
        assert_eq!(result["ok"], true);
        fixture
    }
    fn run(&self, args: &[&str], credential: bool) -> Value {
        let mut child = Command::new(env!("CARGO_BIN_EXE_audb"))
            .arg("--json")
            .args(args)
            .env("XDG_CONFIG_HOME", self.0.path().join("config"))
            .env("XDG_CACHE_HOME", self.0.path().join("cache"))
            .env("AUDB_ROOT_FIXTURE", self.0.path())
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.0.path().join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if credential {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"fixture-password\n")
                .unwrap();
        } else {
            drop(child.stdin.take());
        }
        let output = child.wait_with_output().unwrap();
        assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-password"));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("fixture-password"));
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn registry(&self) -> Vec<u8> {
        fs::read(self.0.path().join("config/audb/devices-v1.json")).unwrap()
    }
}

#[test]
fn provisions_each_device_once_and_keeps_default_and_password_private() {
    let f = Fixture::new();
    let before: Value = serde_json::from_slice(&f.registry()).unwrap();
    let result = f.run(
        &["--device", "phone", "setup-root", "--root-password-stdin"],
        true,
    );
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["deviceId"], "phone");
    assert_eq!(result["data"]["verified"], true);
    assert_eq!(result["data"]["changed"], true);
    let after: Value = serde_json::from_slice(&f.registry()).unwrap();
    assert_eq!(before["defaultDevice"], after["defaultDevice"]);
    assert!(!String::from_utf8(f.registry())
        .unwrap()
        .contains("fixture-password"));
    let key = after["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "phone")
        .unwrap()["sshKey"]
        .as_str()
        .unwrap();
    assert_eq!(
        fs::metadata(key).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(f.0.path().join("root-key").exists());
    assert!(f.0.path().join("user-key").exists());
    let calls = fs::read_to_string(f.0.path().join("calls")).unwrap();
    assert!(!calls.contains("fixture-password"));
    let again = f.run(&["--device", "phone", "setup-root"], false);
    assert_eq!(again["data"]["changed"], false);
    let remaining = fs::read_to_string(f.0.path().join("calls")).unwrap();
    assert!(!remaining[calls.len()..].contains("devel-su"));
    assert!(!remaining.contains("sshd_config"));
}

#[test]
fn rejected_ssh_policy_rolls_back_added_keys_without_registering_root() {
    let f = Fixture::new();
    let before = f.registry();
    fs::write(f.0.path().join("deny-root"), "").unwrap();
    let result = f.run(
        &["--device", "phone", "setup-root", "--root-password-stdin"],
        true,
    );
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "ROOT_ACCESS_REQUIRED");
    assert_eq!(result["data"]["keyCleanup"], true);
    assert_eq!(f.registry(), before);
    assert!(!f.0.path().join("root-key").exists());
    assert!(!f.0.path().join("user-key").exists());
}

#[test]
fn check_only_and_invalid_credentials_never_authorize_keys() {
    let f = Fixture::new();
    let before = f.registry();
    assert_eq!(
        f.run(&["--device", "phone", "setup-root", "--check-only"], false)["ok"],
        false
    );
    assert_eq!(
        f.run(&["--device", "phone", "setup-root"], false)["error"]["code"],
        "ROOT_ACCESS_REQUIRED"
    );
    fs::write(f.0.path().join("bad-password"), "").unwrap();
    assert_eq!(
        f.run(
            &["--device", "phone", "setup-root", "--root-password-stdin"],
            true
        )["error"]["code"],
        "AUTHENTICATION_FAILED"
    );
    assert_eq!(f.registry(), before);
    assert!(!f.0.path().join("root-key").exists());
    assert!(!f.0.path().join("user-key").exists());
}
