//! Explicit developer-device installation of system RPMs, separate from APM.
use crate::{
    transport::{shell_quote, DeviceTransport},
    CoreError, CoreResult,
};
use audb_protocol::{ErrorCode, RootPassword, SystemPackageOptions};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

pub const AGENT_PACKAGE_NAME: &str = "audb-agent";
pub(crate) const QUERY_FORMAT: &str = "%{NAME}\t%{VERSION}\t%{RELEASE}\t%{ARCH}\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PackageInfo {
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) release: String,
    pub(crate) arch: String,
}

trait InstallerTransport {
    fn cleanup_config(&self) -> Option<crate::devices::DeviceConfig> {
        None
    }
    async fn user(&mut self, command: &str) -> CoreResult<String>;
    async fn root(&mut self, command: &str, password: Option<&RootPassword>) -> CoreResult<String>;
    async fn upload(&mut self, remote: &Path, bytes: &[u8]) -> CoreResult<()>;
}
impl InstallerTransport for DeviceTransport {
    fn cleanup_config(&self) -> Option<crate::devices::DeviceConfig> {
        Some(self.config().clone())
    }
    async fn user(&mut self, command: &str) -> CoreResult<String> {
        self.exec(command, false).await
    }
    async fn root(&mut self, command: &str, password: Option<&RootPassword>) -> CoreResult<String> {
        self.exec_privileged(command, password).await
    }
    async fn upload(&mut self, remote: &Path, bytes: &[u8]) -> CoreResult<()> {
        self.upload_bytes(remote, bytes).await
    }
}

/// Install any explicitly supplied system RPM. The validation override applies
/// only to these RPM invocations; system configuration is never modified.
pub async fn install_system_package(
    t: &mut DeviceTransport,
    local: &Path,
    options: SystemPackageOptions,
) -> CoreResult<Value> {
    install_file(t, local, options, None).await
}

/// Restrict device bootstrap to our agent's RPM, using exactly the same installer.
/// The package starts the service; bootstrap also verifies its readiness.
pub async fn setup_device(
    t: &mut DeviceTransport,
    local: &Path,
    options: SystemPackageOptions,
) -> CoreResult<Value> {
    let check_only = options.check_only;
    let mut result = install_file(t, local, options, Some(AGENT_PACKAGE_NAME)).await?;
    if !check_only {
        // Maliit discovers plugins only at startup. This affects the keyboard,
        // without stopping the foreground application or altering user layouts.
        result["inputActivation"] = match t
            .exec("systemctl --user try-restart maliit-server.service", false)
            .await
        {
            Ok(_) => json!({"keyboardRestartRequested":true}),
            Err(e) => {
                json!({"keyboardRestartRequested":false,"error":{"code":e.code,"message":e.message}})
            }
        };
        let mut last_error = None;
        for attempt in 0..10 {
            match crate::physical_input::call(t, json!({"command":"status"})).await {
                Ok(agent) => {
                    // The session service becomes D-Bus-ready before it finishes
                    // loading its QML plugins. Give the bridge time to appear.
                    if attempt < 9 && agent["capabilities"]["text"] != true {
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        continue;
                    }
                    result["agent"] = agent;
                    return Ok(result);
                }
                Err(e) => last_error = Some(e),
            }
            if attempt < 9 {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
        let e = last_error.unwrap();
        return Err(CoreError::new(
            e.code,
            format!("RPM installed, but audb-agent is not ready: {}", e.message),
        )
        .with_data(
            json!({"phase":"agent_verify","rpmTransactionCompleted":true,"installation":result}),
        ));
    }
    Ok(result)
}

async fn install_file(
    t: &mut DeviceTransport,
    local: &Path,
    options: SystemPackageOptions,
    expected: Option<&str>,
) -> CoreResult<Value> {
    if local.extension().and_then(|s| s.to_str()) != Some("rpm") {
        return Err(CoreError::invalid("File must be .rpm"));
    }
    let bytes = tokio::fs::read(local).await.map_err(|e| {
        CoreError::new(
            ErrorCode::NotFound,
            format!("Cannot read RPM {}: {e}", local.display()),
        )
    })?;
    if !bytes.starts_with(&[0xed, 0xab, 0xee, 0xdb]) {
        return Err(CoreError::invalid("File does not have an RPM lead header"));
    }
    install_bytes(t, &bytes, &options, expected).await
}

async fn install_bytes(
    t: &mut impl InstallerTransport,
    bytes: &[u8],
    options: &SystemPackageOptions,
    expected: Option<&str>,
) -> CoreResult<Value> {
    let dir = t
        .user("umask 077 && mktemp -d /tmp/audb-system-rpm.XXXXXX")
        .await?;
    let dir = dir.trim();
    validate_stage(dir)?;
    let remote = format!("{dir}/package.rpm");
    let mut guard = StageCleanup {
        config: t.cleanup_config(),
        command: cleanup_command(dir, &remote),
    };
    let result = async {
        t.upload(Path::new(&remote), bytes).await?;
        let metadata = t.user(&format!("rpm -qp --queryformat {} -- {}", shell_quote(QUERY_FORMAT), shell_quote(&remote))).await?;
        let package = parse_info(&metadata)?;
        if let Some(expected) = expected {
            if package.name != expected { return Err(CoreError::invalid(format!("setup-device requires package '{expected}', got '{}'", package.name))); }
        }
        let arch = t.user("rpm --eval '%{_arch}'").await?;
        let arch = arch.trim();
        if package.arch != "noarch" && package.arch != arch {
            return Err(CoreError::invalid(format!("Package architecture '{}' does not match device '{arch}'", package.arch)));
        }
        // --test uses the same mode and override as the actual transaction.
        let check = t.root(&rpm_command(&remote, options.upgrade, options.reinstall, true), options.root_password.as_ref()).await
            .map_err(|e| phase_error(e, "check", &package))?;
        if options.check_only {
            return Ok(json!({"package": package, "installed": false, "checked": true, "checkOnly": true,
                "mode": if options.upgrade {"upgrade"} else {"install"}, "reinstall": options.reinstall,
                "transactionValidation": "disabled_for_this_transaction", "checkOutput": check}));
        }
        let output = t.root(&rpm_command(&remote, options.upgrade, options.reinstall, false), options.root_password.as_ref()).await
            .map_err(|e| phase_error(e, "install", &package))?;
        let installed = t.root(&format!("rpm -q --queryformat {} -- {}", shell_quote(QUERY_FORMAT), shell_quote(&package.name)), options.root_password.as_ref()).await
            .map_err(|e| phase_error(e, "verify", &package))?;
        if !installed.lines().any(|line| parse_info(line).is_ok_and(|info| info == package)) {
            return Err(CoreError::runtime("RPM returned success but installed package metadata does not match")
                .with_data(json!({"phase":"verify", "rpmTransactionCompleted":true, "package":package, "rpmOutput":output})));
        }
        Ok(json!({"package": package, "installed": true, "checked": true, "checkOnly": false,
            "mode": if options.upgrade {"upgrade"} else {"install"}, "reinstall": options.reinstall,
            "transactionValidation": "disabled_for_this_transaction", "checkOutput": check, "output": output}))
    }.await;
    // Remove only the known uploaded file and its unique directory, never glob.
    let cleanup = t.user(&guard.command).await;
    guard.config = None;
    match result {
        Ok(mut value) => {
            value["stagingCleanup"] = json!(cleanup.is_ok());
            if let Err(e) = cleanup {
                value["cleanupError"] = json!({"message":e.message,"directory":dir});
            }
            Ok(value)
        }
        Err(mut e) => {
            let mut data = e.data.take().unwrap_or_else(|| json!({}));
            data["stagingCleanup"] = json!(cleanup.is_ok());
            if let Err(cleanup) = cleanup {
                data["cleanupError"] = json!({"message":cleanup.message,"directory":dir});
            }
            e.data = Some(data);
            Err(e)
        }
    }
}

fn cleanup_command(dir: &str, remote: &str) -> String {
    format!(
        "rm -f -- {} && rmdir -- {}",
        shell_quote(remote),
        shell_quote(dir)
    )
}
/// Best-effort cleanup when the caller disconnects or its deadline cancels us.
/// No credential is retained: the uploading SSH account owns the staging dir.
pub(crate) struct StageCleanup {
    pub(crate) config: Option<crate::devices::DeviceConfig>,
    pub(crate) command: String,
}
impl Drop for StageCleanup {
    fn drop(&mut self) {
        if let (Some(config), Ok(runtime)) =
            (self.config.take(), tokio::runtime::Handle::try_current())
        {
            let command = self.command.clone();
            runtime.spawn(async move {
                let mut transport = DeviceTransport::new(config);
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    transport.exec(&command, false),
                )
                .await;
            });
        }
    }
}

fn validate_stage(dir: &str) -> CoreResult<()> {
    let suffix = dir
        .strip_prefix("/tmp/audb-system-rpm.")
        .ok_or_else(|| CoreError::runtime("Unexpected RPM staging directory"))?;
    if suffix.len() != 6 || !suffix.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return Err(CoreError::runtime("Invalid RPM staging directory"));
    }
    Ok(())
}
pub(crate) fn parse_info(raw: &str) -> CoreResult<PackageInfo> {
    let parts: Vec<_> = raw.trim_end_matches(['\n', '\r']).split('\t').collect();
    if parts.len() != 4
        || parts.iter().any(|p| {
            p.is_empty()
                || p.starts_with('-')
                || !p
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._+~^-".contains(&c))
        })
    {
        return Err(CoreError::runtime("Invalid RPM metadata"));
    }
    Ok(PackageInfo {
        name: parts[0].into(),
        version: parts[1].into(),
        release: parts[2].into(),
        arch: parts[3].into(),
    })
}
fn rpm_command(path: &str, upgrade: bool, reinstall: bool, test: bool) -> String {
    format!(
        "rpm {}{}{} --undefine=__transaction_validation -- {}",
        if upgrade { "-Uvh" } else { "-ivh" },
        if test { " --test" } else { "" },
        if reinstall { " --replacepkgs" } else { "" },
        shell_quote(path)
    )
}
fn phase_error(mut e: CoreError, phase: &str, package: &PackageInfo) -> CoreError {
    let mut data = e.data.take().unwrap_or_else(|| json!({}));
    data["phase"] = json!(phase);
    data["package"] = json!(package);
    if phase == "verify" {
        data["rpmTransactionCompleted"] = json!(true);
    }
    e.data = Some(data);
    e
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Mock {
        calls: Vec<String>,
        metadata: Option<String>,
        arch: Option<String>,
        fail_check: bool,
        fail_install: bool,
        fail_upload: bool,
    }
    impl InstallerTransport for Mock {
        async fn user(&mut self, command: &str) -> CoreResult<String> {
            self.calls.push(command.into());
            Ok(if command.contains("mktemp") {
                "/tmp/audb-system-rpm.ABC123".into()
            } else if command.contains("rpm -qp") {
                self.metadata
                    .clone()
                    .unwrap_or_else(|| "audb-agent\t0.3.0\t1\taarch64\n".into())
            } else if command.contains("rpm --eval") {
                self.arch.clone().unwrap_or_else(|| "aarch64".into())
            } else {
                String::new()
            })
        }
        async fn root(&mut self, command: &str, _: Option<&RootPassword>) -> CoreResult<String> {
            self.calls.push(command.into());
            if self.fail_check && command.contains("--test")
                || self.fail_install
                    && command.contains("--undefine")
                    && !command.contains("--test")
            {
                return Err(CoreError::new(ErrorCode::RemoteCommandFailed, "RPM failed"));
            }
            Ok(if command.contains("rpm -q ") {
                "audb-agent\t0.3.0\t1\taarch64\n".into()
            } else {
                "ok".into()
            })
        }
        async fn upload(&mut self, _: &Path, _: &[u8]) -> CoreResult<()> {
            if self.fail_upload {
                Err(CoreError::ssh("upload failed"))
            } else {
                Ok(())
            }
        }
    }
    #[tokio::test]
    async fn check_precedes_install_and_verification() {
        let mut t = Mock::default();
        let result = install_bytes(
            &mut t,
            &[],
            &SystemPackageOptions::default(),
            Some(AGENT_PACKAGE_NAME),
        )
        .await
        .unwrap();
        assert_eq!(result["installed"], true);
        assert_eq!(result["stagingCleanup"], true);
        let transactions: Vec<_> = t
            .calls
            .iter()
            .filter(|c| c.contains("--undefine"))
            .collect();
        assert_eq!(transactions.len(), 2);
        assert!(transactions[0].contains("--test"));
        assert!(!transactions[1].contains("--test"));
        assert!(transactions.iter().all(|c| c.contains("-ivh")));
        assert!(t.calls.iter().any(|c| c.starts_with("rpm -q ")));
    }
    #[tokio::test]
    async fn check_only_does_not_install_and_upgrade_is_explicit() {
        let mut t = Mock::default();
        let options = SystemPackageOptions {
            check_only: true,
            upgrade: true,
            ..Default::default()
        };
        let result = install_bytes(&mut t, &[], &options, None).await.unwrap();
        assert_eq!(result["installed"], false);
        let transactions: Vec<_> = t
            .calls
            .iter()
            .filter(|c| c.contains("--undefine"))
            .collect();
        assert_eq!(transactions.len(), 1);
        assert!(transactions[0].contains("-Uvh --test"));
    }
    #[tokio::test]
    async fn failed_preflight_never_installs_and_failure_is_not_retried() {
        for fail_check in [true, false] {
            let mut t = Mock {
                fail_check,
                fail_install: !fail_check,
                ..Default::default()
            };
            let error = install_bytes(&mut t, &[], &SystemPackageOptions::default(), None)
                .await
                .unwrap_err();
            assert_eq!(
                error.data.as_ref().unwrap()["phase"],
                if fail_check { "check" } else { "install" }
            );
            assert_eq!(
                t.calls
                    .iter()
                    .filter(|c| c.contains("--undefine") && !c.contains("--test"))
                    .count(),
                usize::from(!fail_check)
            );
            assert!(t.calls.last().unwrap().contains("rmdir"));
        }
    }
    #[tokio::test]
    async fn wrong_agent_name_or_architecture_fails_before_root() {
        for metadata in ["other\t1\t1\taarch64", "audb-agent\t1\t1\tarmv7hl"] {
            let mut t = Mock {
                metadata: Some(metadata.into()),
                ..Default::default()
            };
            assert!(install_bytes(
                &mut t,
                &[],
                &SystemPackageOptions::default(),
                Some(AGENT_PACKAGE_NAME)
            )
            .await
            .is_err());
            assert!(!t.calls.iter().any(|c| c.contains("--undefine")));
            assert!(t.calls.last().unwrap().contains("rmdir"));
        }
    }
    #[tokio::test]
    async fn upload_failure_cleans_up() {
        let mut t = Mock {
            fail_upload: true,
            ..Default::default()
        };
        assert!(
            install_bytes(&mut t, &[], &SystemPackageOptions::default(), None)
                .await
                .is_err()
        );
        assert!(t.calls.last().unwrap().contains("rmdir"));
    }
    #[test]
    fn metadata_and_paths_cannot_inject_shell_commands() {
        assert!(parse_info("bad;id\t1\t1\taarch64").is_err());
        assert!(validate_stage("/tmp/audb-system-rpm.ABC123;id").is_err());
        assert!(rpm_command("a'b.rpm", false, false, false).ends_with("'a'\\''b.rpm'"));
    }
}
