//! Bounded, read-only readiness checks. Never run setup, input, or root bootstrap.
use crate::{
    devices::DeviceKind, physical_input, qmp::QmpClient, transport::DeviceTransport, CoreError,
    CoreResult,
};
use audb_protocol::PROTOCOL_VERSION;
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, future::Future, time::Duration};

const AGENT_PROTOCOL: u64 = 3;
const INVENTORY: &str = r#"# audb doctor inventory v1
uid=$(id -u) || exit 1
printf 'uid\t%s\narchitecture\t%s\n' "$uid" "$(uname -m)"
if test -S /run/user/$uid/dbus/user_bus_socket; then printf 'session\ttrue\n'; else printf 'session\tfalse\n'; fi
for tool in gdbus rpm journalctl; do
  if command -v "$tool" >/dev/null 2>&1; then printf 'tool.%s\ttrue\n' "$tool"; else printf 'tool.%s\tfalse\n' "$tool"; fi
done
if command -v rpm >/dev/null 2>&1; then printf 'agentRpm\t'; rpm -q --qf '%{VERSION}-%{RELEASE}\n' audb-agent 2>/dev/null || true; fi
"#;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    /// passed, failed, warning or skipped
    pub status: String,
    pub code: Option<String>,
    pub message: Option<String>,
    pub fix: Option<String>,
    pub data: Value,
}
impl Check {
    fn passed(name: &str, data: Value) -> Self {
        Self {
            name: name.into(),
            status: "passed".into(),
            code: None,
            message: None,
            fix: None,
            data,
        }
    }
    fn problem(name: &str, status: &str, code: &str, message: &str, fix: &str) -> Self {
        Self {
            name: name.into(),
            status: status.into(),
            code: Some(code.into()),
            message: Some(message.into()),
            fix: Some(fix.into()),
            data: Value::Null,
        }
    }
    fn ok(&self) -> bool {
        self.status == "passed"
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    /// audb implements this operation for this target kind.
    pub supported: bool,
    /// Required backend responded; null means this operation was not probed.
    pub available: Option<bool>,
    /// Known prerequisites are satisfied; null means not probed. App/path policies still apply.
    pub ready: Option<bool>,
    pub backend: Option<String>,
    pub geometry: Option<Value>,
    pub reason_code: Option<String>,
    pub reason: Option<String>,
    pub fix: Option<String>,
    pub limitations: Vec<String>,
}
impl Capability {
    fn new(supported: bool, available: bool, ready: bool, backend: &str) -> Self {
        Self {
            supported,
            available: Some(available),
            ready: Some(ready),
            backend: (!backend.is_empty()).then(|| backend.into()),
            geometry: None,
            reason_code: None,
            reason: None,
            fix: None,
            limitations: Vec::new(),
        }
    }
    fn blocked(mut self, code: &str, reason: &str, fix: &str) -> Self {
        self.ready = Some(false);
        self.reason_code = Some(code.into());
        self.reason = Some(reason.into());
        self.fix = (!fix.is_empty()).then(|| fix.into());
        self
    }
    fn unknown(mut self) -> Self {
        self.available = None;
        self.ready = None;
        self
    }
    fn notes(mut self, notes: &[&str]) -> Self {
        self.limitations = notes.iter().map(|s| (*s).into()).collect();
        self
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub report_version: u32,
    pub device: Value,
    pub healthy: bool,
    pub checks: Vec<Check>,
    pub capabilities: BTreeMap<String, Capability>,
}

async fn probe(
    name: &str,
    seconds: u64,
    future: impl Future<Output = CoreResult<Value>>,
    fix: &str,
) -> Check {
    match tokio::time::timeout(Duration::from_secs(seconds), future).await {
        Ok(Ok(data)) => Check::passed(name, data),
        Ok(Err(e)) => Check::problem(name, "failed", &e.code.to_string(), &e.message, fix),
        Err(_) => Check::problem(
            name,
            "failed",
            "CHECK_TIMEOUT",
            "Read-only check timed out; no repair was attempted",
            fix,
        ),
    }
}

fn inventory(raw: &str) -> CoreResult<Value> {
    let mut fields = serde_json::Map::new();
    for line in raw.lines() {
        if let Some((key, value)) = line.split_once('\t') {
            fields.insert(key.into(), json!(value));
        }
    }
    let uid = fields
        .get("uid")
        .and_then(Value::as_str)
        .and_then(|v| v.parse::<u32>().ok())
        .ok_or_else(|| CoreError::runtime("SSH inventory has no valid UID"))?;
    if fields
        .get("architecture")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(CoreError::runtime("SSH inventory has no architecture"));
    }
    fields.insert("uid".into(), json!(uid));
    Ok(Value::Object(fields))
}

pub async fn inspect(t: &mut DeviceTransport, qmp: Option<&mut QmpClient>) -> Report {
    // QEMU's QMP endpoint normally permits only one client. Release our idle
    // cached connection before opening the disposable read-only probe. Device
    // operations are serialized, so no input action is in flight here.
    if let Some(client) = qmp {
        client.disconnect();
    }
    let config = t.config().clone();
    let emulator = config.kind == DeviceKind::Emulator;
    let ssh_fix = "Check device connectivity, developer-mode SSH, the registered user/key and known_hosts. Verify access with ssh first; audb does not accept unknown host keys.";
    let setup_fix = format!("Run audb --device {} setup-device with the current signed audb-agent RPM; use --upgrade for an existing package.", config.id);
    let ssh = probe(
        "ssh",
        6,
        async { inventory(&t.exec(INVENTORY, false).await?) },
        ssh_fix,
    )
    .await;
    let mut checks = Vec::new();
    let agent;
    let permissions;
    let display;
    let apps;
    let packages;
    let files;
    let root;
    if ssh.ok() {
        agent = probe(
            "agent",
            10,
            async {
                let data = physical_input::call(t, json!({"command":"status"})).await?;
                if data["protocolVersion"] != AGENT_PROTOCOL {
                    return Err(CoreError::new(
                        audb_protocol::ErrorCode::ProtocolMismatch,
                        format!(
                            "Agent protocol {AGENT_PROTOCOL} required, got {}",
                            data["protocolVersion"]
                        ),
                    ));
                }
                Ok(data)
            },
            &setup_fix,
        )
        .await;
        permissions = if agent.ok() {
            probe(
                "permissions",
                5,
                async {
                    let data =
                        physical_input::call(t, json!({"command":"permission_capabilities"}))
                            .await?;
                    if data["permissions"] != true || data["promptSemantics"] != "aurora-boolean" {
                        return Err(CoreError::runtime(
                            "Aurora permission API/semantics are not supported",
                        ));
                    }
                    Ok(data)
                },
                "Check sailjaild and the Aurora version; upgrade audb-agent if its API is older.",
            )
            .await
        } else {
            skipped("permissions", "Agent check failed", &setup_fix)
        };
        display = probe(
            "display",
            4,
            crate::display::status(t),
            "Check the MCE system service and system D-Bus access.",
        )
        .await;
        apps = probe(
            "apps",
            4,
            async { Ok(json!({"runningCount":crate::app::list(t).await?.len()})) },
            "Check RuntimeManager and system D-Bus access.",
        )
        .await;
        packages = probe(
            "packages",
            4,
            async {
                Ok(json!({"packageCount":crate::system::package_list(t, None).await?["count"]}))
            },
            "Check APM and system D-Bus access.",
        )
        .await;
        files = probe(
            "files",
            4,
            async {
                let bytes = t
                    .download_bytes(std::path::Path::new("/etc/os-release"))
                    .await?;
                let text =
                    String::from_utf8(bytes).map_err(|e| CoreError::runtime(e.to_string()))?;
                let version = text
                    .lines()
                    .find_map(|line| line.strip_prefix("VERSION_ID="))
                    .map(|v| v.trim_matches(['\'', '"']));
                Ok(json!({"readVerified":true,"osVersion":version}))
            },
            "Enable the SSH SFTP subsystem and check file read permissions.",
        )
        .await;
        root = if config.root_user.is_some() {
            probe("rootSsh", 4, async {
                if t.exec("id -u", true).await? != "0" { return Err(CoreError::runtime("Configured root SSH account is not UID 0")); }
                Ok(json!({"uid":0}))
            }, "Check the registered root SSH account/key. Agent-backed input and permissions do not require root SSH.").await
        } else {
            skipped("rootSsh", "Root SSH is not configured", "Register --root-user only if root SSH is available. Ordinary agent commands do not need it.")
        };
    } else {
        agent = skipped("agent", "SSH check failed", ssh_fix);
        permissions = skipped("permissions", "SSH check failed", ssh_fix);
        display = skipped("display", "SSH check failed", ssh_fix);
        apps = skipped("apps", "SSH check failed", ssh_fix);
        packages = skipped("packages", "SSH check failed", ssh_fix);
        files = skipped("files", "SSH check failed", ssh_fix);
        root = skipped("rootSsh", "SSH check failed", ssh_fix);
    }
    let qmp = if let Some(options) = config.emulator.as_ref() {
        // Use a disposable client so a timed-out read-only probe cannot poison the input session.
        probe(
            "qmp",
            4,
            async {
                let mut client = QmpClient::new(&options.qmp_socket);
                let status = client.execute("query-status", None).await?;
                let commands = client.execute("query-commands", None).await?;
                let display = if commands.as_array().is_some_and(|v| v.iter().any(|c| c["name"]=="screendump")) {
                    match crate::input::Geometry::query(&mut client).await {
                        Ok(g) => json!({"width":g.width,"height":g.height,"coordinateSpace":"qmp-primary-display","source":"QMP.screendump"}),
                        Err(e) => json!({"error":{"code":e.code,"message":e.message}}),
                    }
                } else { Value::Null };
                Ok(json!({"status":status,"commands":commands,"display":display}))
            },
            "Start the emulator and check its registered QMP socket.",
        )
        .await
    } else {
        skipped("qmp", "Physical devices do not use QMP", "")
    };
    let capabilities = capabilities(
        emulator,
        &ssh,
        &agent,
        &permissions,
        &display,
        &apps,
        &packages,
        &files,
        &root,
        &qmp,
    );
    if ssh.ok() {
        checks.push(Check::passed("identity", json!({"uid":ssh.data["uid"],"architecture":ssh.data["architecture"],"osVersion":files.data["osVersion"],"agentRpm":ssh.data["agentRpm"]})));
        checks.push(if ssh.data["session"] == "true" {
            Check::passed("userSession", json!({"busSocketPresent":true}))
        } else {
            Check::problem(
                "userSession",
                "warning",
                "SESSION_UNAVAILABLE",
                "No graphical user bus socket was found for the SSH UID",
                "Log in to the graphical session using the registered SSH user.",
            )
        });
        if agent.ok() && agent.data["capabilities"]["text"] == true {
            checks.push(if agent.data["input"]["active"] == true {
                Check::passed("inputFocus", json!({"active":true}))
            } else { Check::problem("inputFocus", "warning", "INPUT_NOT_FOCUSED", "Unicode backend is available; no compatible editor is focused", "Tap an editable field before text. If an app was open during setup, restart that app once.") });
        }
    }
    checks.splice(
        0..0,
        [
            ssh,
            agent,
            permissions,
            display,
            apps,
            packages,
            files,
            root,
            qmp,
        ],
    );
    let healthy = checks.iter().all(|check| check.status != "failed");
    Report {
        report_version: 1,
        device: json!({"id":config.id,"kind":config.kind,"host":config.host,"sshUser":config.ssh_user,
        "versions":{"cli":env!("CARGO_PKG_VERSION"),"protocol":PROTOCOL_VERSION,"expectedAgentProtocol":AGENT_PROTOCOL}}),
        healthy,
        checks,
        capabilities,
    }
}

fn skipped(name: &str, message: &str, fix: &str) -> Check {
    Check::problem(name, "skipped", "CHECK_SKIPPED", message, fix)
}

#[allow(clippy::too_many_arguments)]
fn capabilities(
    emulator: bool,
    ssh: &Check,
    agent: &Check,
    permissions: &Check,
    display: &Check,
    apps: &Check,
    packages: &Check,
    files: &Check,
    root: &Check,
    qmp: &Check,
) -> BTreeMap<String, Capability> {
    let mut result = BTreeMap::new();
    let backend = |check: &Check, name: &str| {
        let cap = Capability::new(true, check.ok(), check.ok(), name);
        if check.ok() {
            cap
        } else {
            cap.blocked(
                check.code.as_deref().unwrap_or("CAPABILITY_UNAVAILABLE"),
                check.message.as_deref().unwrap_or("Check failed"),
                check.fix.as_deref().unwrap_or(""),
            )
        }
    };
    result.insert("shell".into(), backend(ssh, "ssh"));
    result.insert("files".into(), backend(files, "sftp").notes(&["Read access to /etc/os-release was probed; target path read/write policies and free space still apply."]));
    result.insert("apps".into(), backend(apps, "RuntimeManager").notes(&["Read-only application listing was probed; launch/stop depend on the selected application."]));
    result.insert("packages".into(), backend(packages, "APM").notes(&["Read-only package listing was probed; installation also requires a valid RPM, policy and disk space.","RPM contents are streamed via SFTP; installation waits for matching APM registration."]));
    result.insert("permissions".into(), backend(permissions, "sailjaild1").notes(&["Service interface and signatures were probed; per-application permissions are checked during each call."]));
    result.insert("displayStatus".into(), backend(display, "MCE").notes(&["Read-only status was probed; policy changes require root SSH in the current display implementation."]));
    result.insert(
        "displayControl".into(),
        if display.ok() && root.ok() {
            Capability::new(true, true, true, "MCE")
                .notes(&["MCE status and root SSH were probed; display changes were not executed."])
        } else {
            Capability::new(true, false, false, "MCE").blocked(
                "ROOT_ACCESS_REQUIRED",
                "Current display control requires MCE and working root SSH",
                "Check displayStatus and configure root SSH.",
            )
        },
    );
    result.insert("rootShell".into(), backend(root, "ssh-root"));
    result.insert("logs".into(), if ssh.data["tool.journalctl"] == "true" && root.ok() {
        Capability::new(true, true, true, "journalctl").notes(&["Root SSH and journalctl presence were probed; journal source/filter access is checked by logs."])
    } else { Capability::new(true, false, false, "journalctl").blocked("ROOT_ACCESS_REQUIRED", "Current logs implementation requires journalctl and working root SSH", "Configure root SSH; audb-agent does not yet expose log reading.") });
    let qmp_has = |name: &str| {
        qmp.ok()
            && qmp.data["commands"]
                .as_array()
                .is_some_and(|commands| commands.iter().any(|v| v["name"] == name))
    };
    let qmp_running = qmp.ok() && qmp.data["status"]["running"] == true;
    let screen_ready = display.ok()
        && matches!(display.data["display"].as_str(), Some("on" | "dim"))
        && display.data["lockMode"] == "unlocked"
        && display.data["touchPolicy"] == "enabled";
    for name in ["tap", "swipe", "key", "text", "screenshot"] {
        let use_agent = !emulator
            || matches!(name, "key" | "text")
                && agent.ok()
                && agent.data["capabilities"][name] == true;
        let cap = if use_agent {
            let available = agent.ok() && agent.data["capabilities"][name] == true;
            let mut cap = Capability::new(
                true,
                available,
                available,
                if name == "text" {
                    "maliit"
                } else if name == "screenshot" {
                    "lipstick"
                } else {
                    "uinput"
                },
            );
            if !available {
                cap = cap.blocked(
                    agent.code.as_deref().unwrap_or("CAPABILITY_UNAVAILABLE"),
                    agent
                        .message
                        .as_deref()
                        .unwrap_or("Agent backend is unavailable"),
                    "Run doctor and setup-device; check the user session and agent capabilities.",
                );
            } else if name == "text" && agent.data["input"]["active"] != true {
                cap = cap.blocked(
                    "INPUT_NOT_FOCUSED",
                    "No compatible editor is focused",
                    "Tap an editable field before text.",
                );
            } else if matches!(name, "tap" | "swipe")
                && (!screen_ready || agent.data["display"].is_null())
            {
                cap = cap.blocked("DISPLAY_NOT_READY", "Touch readiness requires graphical geometry, an awake screen and an unlocked session", "Wake/unlock the device and check display status; PIN entry remains manual.");
            }
            if name == "screenshot" {
                cap = cap.notes(&["Geometry/backend readiness was probed without capturing an image; screen-off or locked content may differ."]);
            }
            if name == "text" {
                cap = cap.notes(&["Unicode uses the focused compatible Maliit editor; secure/custom editors may reject commits."]);
            }
            cap
        } else {
            let available = qmp_has(if name == "screenshot" {
                "screendump"
            } else {
                "input-send-event"
            });
            let mut cap = Capability::new(true, available, available && qmp_running, "qmp");
            if !available {
                cap = cap.blocked(
                    "QMP_UNAVAILABLE",
                    "Required QMP command is unavailable",
                    "Start the emulator and check its QMP socket.",
                );
            } else if !qmp_running {
                cap = cap.blocked(
                    "EMULATOR_NOT_RUNNING",
                    "QMP is connected but the emulator is not running",
                    "Resume/start the emulator.",
                );
            }
            if matches!(name, "tap" | "swipe" | "screenshot") {
                cap.geometry =
                    (!qmp.data["display"].is_null()).then(|| qmp.data["display"].clone());
            }
            if matches!(name, "tap" | "swipe")
                && available
                && qmp_running
                && qmp.data["display"]["width"].as_u64().is_none()
            {
                cap = cap.blocked("GEOMETRY_UNAVAILABLE", "Cannot determine QMP display dimensions; touch is not sent with guessed coordinates", "Check QMP screendump support and doctor geometry.");
            }
            if name == "text" {
                cap = cap.notes(&["ASCII only and requires a matching guest keyboard layout; install audb-agent for Unicode."]);
            }
            if name == "screenshot" {
                cap = cap.notes(&["QMP screendump availability was probed; legacy capture can fall back to a host window or Lipstick if capture fails."]);
            }
            cap
        };
        result.insert(name.into(), cap);
    }
    for name in ["clipboard", "uiTree", "location", "sensors"] {
        let supported = emulator && matches!(name, "location" | "sensors");
        let cap = Capability::new(
            supported,
            false,
            false,
            if supported { "emulator-api" } else { "" },
        )
        .blocked(
            if supported {
                "NOT_PROBED"
            } else {
                "UNSUPPORTED"
            },
            if supported {
                "Emulator API exists but was not probed by this report"
            } else {
                "This operation is not implemented for this target"
            },
            "",
        );
        result.insert(name.into(), if supported { cap.unknown() } else { cap });
    }
    for name in [
        "info",
        "perf",
        "crash",
        "sandbox",
        "network",
        "open",
        "systemPackageInstall",
    ] {
        result.insert(name.into(), Capability::new(true, false, false, "")
            .blocked("NOT_PROBED", "This operation is implemented but its requirements were not checked by this report", "Consult the command help and its returned error; parity auditing remains planned.")
            .unknown());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_agent(active: bool) -> Check {
        Check::passed(
            "agent",
            json!({"protocolVersion":3,"display":{"width":2000,"height":1200},
            "capabilities":{"tap":true,"swipe":true,"key":true,"text":true,"screenshot":true},"input":{"active":active}}),
        )
    }
    fn evaluate(
        emulator: bool,
        ssh: &Check,
        agent: &Check,
        display: &Check,
        qmp: &Check,
    ) -> BTreeMap<String, Capability> {
        let absent = skipped("absent", "Not available", "Check setup");
        capabilities(
            emulator, ssh, agent, &absent, display, &absent, &absent, &absent, &absent, qmp,
        )
    }
    #[test]
    fn focus_and_lock_block_input_without_hiding_available_backends() {
        let ssh = Check::passed("ssh", json!({}));
        let display = Check::passed("display", json!({"display":"on","lockMode":"locked"}));
        let caps = evaluate(
            false,
            &ssh,
            &good_agent(false),
            &display,
            &skipped("qmp", "physical", ""),
        );
        assert!(
            caps["text"].supported
                && caps["text"].available == Some(true)
                && caps["text"].ready == Some(false)
        );
        assert_eq!(
            caps["text"].reason_code.as_deref(),
            Some("INPUT_NOT_FOCUSED")
        );
        assert!(caps["tap"].available == Some(true) && caps["tap"].ready == Some(false));
        assert_eq!(
            caps["tap"].reason_code.as_deref(),
            Some("DISPLAY_NOT_READY")
        );
        assert!(caps["key"].ready == Some(true));
        assert!(caps["screenshot"].ready == Some(true));
        assert!(!caps["uiTree"].supported);
        assert!(!caps["sensors"].supported);
    }
    #[test]
    fn ssh_failure_does_not_hide_independent_qmp_or_claim_unicode() {
        let absent = Check::problem("ssh", "failed", "SSH_ERROR", "offline", "Check network");
        let qmp = Check::passed(
            "qmp",
            json!({"status":{"running":true},"display":{"width":360,"height":800},
            "commands":[{"name":"input-send-event"},{"name":"screendump"}]}),
        );
        let caps = evaluate(true, &absent, &absent, &absent, &qmp);
        assert!(caps["shell"].ready == Some(false));
        assert!(caps["tap"].ready == Some(true) && caps["screenshot"].ready == Some(true));
        assert_eq!(caps["text"].backend.as_deref(), Some("qmp"));
        assert!(caps["text"]
            .limitations
            .iter()
            .any(|s| s.contains("ASCII only")));
    }
    #[test]
    fn paused_qmp_and_missing_command_are_distinct() {
        let absent = skipped("absent", "Not available", "");
        let qmp = Check::passed(
            "qmp",
            json!({"status":{"running":false},"commands":[{"name":"input-send-event"}]}),
        );
        let caps = evaluate(true, &absent, &absent, &absent, &qmp);
        assert!(caps["tap"].available == Some(true) && caps["tap"].ready == Some(false));
        assert_eq!(
            caps["tap"].reason_code.as_deref(),
            Some("EMULATOR_NOT_RUNNING")
        );
        assert!(caps["screenshot"].available == Some(false));
        assert_eq!(
            caps["screenshot"].reason_code.as_deref(),
            Some("QMP_UNAVAILABLE")
        );
    }
    #[test]
    fn emulator_uses_available_agent_text_even_without_focus() {
        let absent = skipped("absent", "Not available", "");
        let caps = evaluate(true, &absent, &good_agent(false), &absent, &absent);
        assert_eq!(caps["text"].backend.as_deref(), Some("maliit"));
        assert!(caps["text"].ready == Some(false));
        assert_eq!(caps["key"].backend.as_deref(), Some("uinput"));
        assert!(caps["key"].ready == Some(true));
    }
    #[test]
    fn malformed_inventory_cannot_report_success() {
        assert!(inventory("uid\twrong\narchitecture\taarch64").is_err());
        assert!(inventory("uid\t100000").is_err());
        assert_eq!(
            inventory("uid\t100000\narchitecture\taarch64\nsession\tfalse").unwrap()["uid"],
            100000
        );
    }
    #[tokio::test]
    async fn check_timeout_is_explicit_and_does_not_retry() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let check = probe(
            "slow",
            0,
            async {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::future::pending::<CoreResult<Value>>().await
            },
            "Check service",
        )
        .await;
        assert_eq!(check.code.as_deref(), Some("CHECK_TIMEOUT"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(check.fix.as_deref(), Some("Check service"));
    }
    #[tokio::test]
    async fn qmp_probe_releases_cached_client_and_remains_read_only_without_guest_ssh() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("qmp.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let mut log = Vec::new();
            // Like real QEMU, serve one QMP connection at a time. The second
            // handshake cannot proceed until the cached client releases the first.
            for count in [2, 4] {
                let (stream, _) = listener.accept().await.unwrap();
                let mut io = BufReader::new(stream);
                io.get_mut().write_all(b"{\"QMP\":{}}\n").await.unwrap();
                for _ in 0..count {
                    let mut line = String::new();
                    io.read_line(&mut line).await.unwrap();
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let command = request["execute"].as_str().unwrap();
                    log.push(command.to_owned());
                    let data = match command {
                        "qmp_capabilities" => json!({}),
                        "query-status" => json!({"running":true,"status":"running"}),
                        "query-commands" => {
                            json!([{"name":"input-send-event"},{"name":"screendump"}])
                        }
                        "screendump" => {
                            let mut png =
                                include_bytes!("../../audb-agent/tests/one-pixel.png").to_vec();
                            png[16..20].copy_from_slice(&360u32.to_be_bytes());
                            png[20..24].copy_from_slice(&800u32.to_be_bytes());
                            std::fs::write(request["arguments"]["filename"].as_str().unwrap(), png)
                                .unwrap();
                            json!({})
                        }
                        _ => panic!("Unexpected modifying QMP command: {command}"),
                    };
                    io.get_mut()
                        .write_all(
                            format!("{}\n", json!({"return":data,"id":request["id"]})).as_bytes(),
                        )
                        .await
                        .unwrap();
                }
                if count == 2 {
                    let mut unexpected = String::new();
                    assert_eq!(
                        tokio::time::timeout(Duration::from_secs(5), io.read_line(&mut unexpected))
                            .await
                            .unwrap()
                            .unwrap(),
                        0
                    );
                }
            }
            log
        });
        let offline = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = offline.local_addr().unwrap().port();
        drop(offline);
        let config = crate::config::EmulatorConfig {
            host: "127.0.0.1".into(),
            ssh_port: port,
            qmp_socket: socket,
            ..Default::default()
        };
        let mut backend = crate::DeviceBackend::new(config.into());
        backend
            .qmp
            .as_mut()
            .unwrap()
            .execute("query-status", None)
            .await
            .unwrap();
        let audb_protocol::CommandOutput::Json(report) = backend
            .execute(audb_protocol::Command::Doctor)
            .await
            .unwrap()
        else {
            panic!("Expected JSON report")
        };
        assert_eq!(report["healthy"], false);
        assert_eq!(report["capabilities"]["shell"]["ready"], false);
        assert_eq!(report["capabilities"]["tap"]["ready"], true);
        assert_eq!(report["capabilities"]["text"]["backend"], "qmp");
        assert_eq!(report["capabilities"]["sensors"]["available"], Value::Null);
        assert_eq!(
            server.await.unwrap(),
            [
                "qmp_capabilities",
                "query-status",
                "qmp_capabilities",
                "query-status",
                "query-commands",
                "screendump"
            ]
        );
    }
}
