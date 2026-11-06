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

/// Install an explicitly supplied system RPM with native signature validation.
/// An explicitly pinned developer key uses a private process configuration.
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
    let root_password = options.root_password.clone();
    let mut result = install_file(t, local, options, Some(AGENT_PACKAGE_NAME)).await?;
    if !check_only {
        // The first %post start may precede certificate registration/loading.
        t.exec_privileged(
            "systemctl reset-failed audb-agent.service && systemctl restart audb-agent.service",
            root_password.as_ref(),
        )
        .await
        .map_err(|e| {
            CoreError::new(e.code, e.message).with_data(
            json!({"phase":"agent_start", "rpmTransactionCompleted":true, "installation":result})
        )
        })?;
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
    if let Some(key) = &options.developer_key_id {
        if parse_developer_key_id(key).as_ref() != Ok(key) {
            return Err(CoreError::invalid(
                "Developer key ID must be lowercase hexadecimal (40–128 digits, even length)",
            ));
        }
    }
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
        let platform_archs =
            crate::rpm_arch::parse_platform_archs(&t.user(crate::rpm_arch::ARCH_QUERY).await?)?;
        crate::rpm_arch::ensure_package_arch(&package.arch, &platform_archs)?;
        // --test uses the same signature policy as the actual transaction.
        let check = t.root(&rpm_command(&remote, options, true), options.root_password.as_ref()).await
            .map_err(|e| phase_error(e, "check", &package))?;
        if options.check_only {
            return Ok(json!({"package": package, "installed": false, "checked": true, "checkOnly": true,
                "mode": if options.upgrade {"upgrade"} else {"install"}, "reinstall": options.reinstall,
                "transactionValidation": "enabled", "developerKeyId": options.developer_key_id, "checkOutput": check}));
        }
        let output = t.root(&rpm_command(&remote, options, false), options.root_password.as_ref()).await
            .map_err(|e| phase_error(e, "install", &package))?;
        let installed = t.root(&format!("rpm -q --queryformat {} -- {}", shell_quote(QUERY_FORMAT), shell_quote(&package.name)), options.root_password.as_ref()).await
            .map_err(|e| phase_error(e, "verify", &package))?;
        if !installed.lines().any(|line| parse_info(line).is_ok_and(|info| info == package)) {
            return Err(CoreError::runtime("RPM returned success but installed package metadata does not match")
                .with_data(json!({"phase":"verify", "rpmTransactionCompleted":true, "package":package, "rpmOutput":output})));
        }
        let certificate_loading = t.root(
            "if test -d /sys/kernel/security/ima; then /usr/libexec/ima/load-certificates 2>&1; else printf 'IMA is not enabled\\n'; fi",
            options.root_password.as_ref(),
        ).await.map_err(|e| phase_error(e, "certificate_load", &package))?;
        Ok(json!({"package": package, "installed": true, "checked": true, "checkOnly": false,
            "mode": if options.upgrade {"upgrade"} else {"install"}, "reinstall": options.reinstall,
            "transactionValidation": "enabled", "developerKeyId": options.developer_key_id, "checkOutput": check, "output": output, "certificateLoaderOutput": certificate_loading}))
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
/// Validate the full public certificate key ID, never a short suffix. The OS
/// still verifies the CMS and checks revocation; this value is not a signature.
pub fn parse_developer_key_id(value: &str) -> Result<String, String> {
    if !(40..=128).contains(&value.len())
        || !value.len().is_multiple_of(2)
        || !value.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(
            "Expected the full certificate key ID (40–128 hexadecimal digits, even length)".into(),
        );
    }
    Ok(value.to_ascii_lowercase())
}

fn rpm_command(path: &str, options: &SystemPackageOptions, test: bool) -> String {
    let rpm = format!(
        "rpm {}{}{} -- {}",
        if options.upgrade { "-Uvh" } else { "-ivh" },
        if test { " --test" } else { "" },
        if options.reinstall {
            " --replacepkgs"
        } else {
            ""
        },
        shell_quote(path)
    );
    match &options.developer_key_id {
        Some(key) => format!(
            "sh -c {} audb-rpm {} {}",
            shell_quote(include_str!("developer_rpm.sh")),
            shell_quote(key),
            rpm
        ),
        None => format!("env -u RPMSIGN_EXTERNAL_ROOTFS {rpm}"),
    }
}
fn phase_error(mut e: CoreError, phase: &str, package: &PackageInfo) -> CoreError {
    let mut data = e.data.take().unwrap_or_else(|| json!({}));
    data["phase"] = json!(phase);
    data["package"] = json!(package);
    if phase == "verify" || phase == "certificate_load" {
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
        installed: Option<String>,
        fail_check: bool,
        fail_install: bool,
        fail_upload: bool,
        fail_loader: bool,
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
            if self.fail_loader && command.contains("/usr/libexec/ima/load-certificates") {
                return Err(CoreError::runtime("loader unavailable"));
            }
            if self.fail_check && command.contains("--test")
                || self.fail_install
                    && (command.contains("rpm -ivh") || command.contains("rpm -Uvh"))
                    && !command.contains("--test")
            {
                return Err(CoreError::new(ErrorCode::RemoteCommandFailed, "RPM failed"));
            }
            Ok(if command.contains("rpm -q ") {
                self.installed
                    .clone()
                    .unwrap_or_else(|| "audb-agent\t0.3.0\t1\taarch64\n".into())
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
            .filter(|c| c.contains("rpm -ivh") || c.contains("rpm -Uvh"))
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
            .filter(|c| c.contains("rpm -ivh") || c.contains("rpm -Uvh"))
            .collect();
        assert_eq!(transactions.len(), 1);
        assert!(transactions[0].contains("-Uvh --test"));
        assert!(!t.calls.iter().any(|c| c.contains("load-certificates")));
    }

    #[tokio::test]
    async fn certificate_loading_follows_verified_install_and_preserves_partial_failure() {
        let mut t = Mock {
            fail_loader: true,
            ..Default::default()
        };
        let error = install_bytes(&mut t, &[], &SystemPackageOptions::default(), None)
            .await
            .unwrap_err();
        let data = error.data.unwrap();
        assert_eq!(data["phase"], "certificate_load");
        assert_eq!(data["rpmTransactionCompleted"], true);
        assert_eq!(data["stagingCleanup"], true);
        let verify = t
            .calls
            .iter()
            .position(|c| c.starts_with("rpm -q "))
            .unwrap();
        let loader = t
            .calls
            .iter()
            .position(|c| c.contains("load-certificates"))
            .unwrap();
        assert!(verify < loader);
    }

    #[tokio::test]
    async fn invalid_developer_key_never_reaches_device() {
        for key in ["44b61e10", "abcd; id", "", &"a".repeat(41), &"A".repeat(64)] {
            let mut t = Mock::default();
            let options = SystemPackageOptions {
                developer_key_id: Some(key.into()),
                ..Default::default()
            };
            assert!(install_bytes(&mut t, &[], &options, None).await.is_err());
            assert!(t.calls.is_empty());
        }
        assert_eq!(
            parse_developer_key_id(&"A".repeat(64)).unwrap(),
            "a".repeat(64)
        );
    }

    #[test]
    fn native_validation_is_never_disabled() {
        let mut options = SystemPackageOptions::default();
        assert!(rpm_command("package.rpm", &options, false)
            .starts_with("env -u RPMSIGN_EXTERNAL_ROOTFS rpm -ivh"));
        options.developer_key_id = Some("a".repeat(64));
        for check in [false, true] {
            let command = rpm_command("a'b.rpm", &options, check);
            assert!(command.contains("RPMSIGN_EXTERNAL_ROOTFS"));
            assert!(!command.contains("--undefine"));
            assert!(!command.contains("--nosignature"));
            assert_eq!(command.contains("--test"), check);
            assert!(command.ends_with("'a'\\''b.rpm'"));
        }
    }

    // Execute the real shell wrapper against an isolated configuration and a
    // fake rpm. This checks policy preservation, exit propagation and cleanup,
    // not merely the spelling of a generated command.
    #[test]
    fn developer_configuration_is_private_preserved_and_cleaned_on_failure() {
        use std::process::Command;
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("source.ini");
        let record = temp.path().join("record");
        let original = "[main]\ndatabase_path = /var/lib/rpmsign-external/db.sqlite\nroot_ca_cert_path = /etc/keys/packages/ca.pem\nrevocation_conf = /etc/keys/packages/revocation.conf\nsystem_developer_keyid_path = /etc/keys/packages/system-developer-keyid\n[other]\nuntouched = yes\n";
        std::fs::write(&config, original).unwrap();
        let script = include_str!("developer_rpm.sh").replace(
            "/etc/rpmsign-external/rpmsign-external.conf >",
            &format!("{} >", shell_quote(config.to_str().unwrap())),
        );
        let probe = r#"set -eu
printf '%s\n' "$RPMSIGN_EXTERNAL_ROOTFS" > "$1"
cat "$RPMSIGN_EXTERNAL_ROOTFS/etc/rpmsign-external/rpmsign-external.conf" >> "$1"
cat "$RPMSIGN_EXTERNAL_ROOTFS/keyid" >> "$1"
exit 17"#;
        let output = Command::new("sh")
            .args([
                "-c",
                &script,
                "audb-rpm",
                &"a".repeat(64),
                "sh",
                "-c",
                probe,
                "probe",
            ])
            .arg(&record)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(17), "{:?}", output);
        assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
        let observed = std::fs::read_to_string(&record).unwrap();
        let mut lines = observed.lines();
        let private = lines.next().unwrap();
        assert!(!Path::new(private).exists());
        assert!(observed.contains("database_path = /var/lib/rpmsign-external/db.sqlite"));
        assert!(observed.contains("root_ca_cert_path = /etc/keys/packages/ca.pem"));
        assert!(observed.contains("revocation_conf = /etc/keys/packages/revocation.conf"));
        assert!(observed.contains(&format!("system_developer_keyid_path = {private}/keyid")));
        assert!(observed.contains("untouched = yes"));
        assert_eq!(observed.lines().last().unwrap(), "a".repeat(64));

        for bad in [
            "[main]\n",
            "[wrong]\nsystem_developer_keyid_path=x\n",
            "[main]\nsystem_developer_keyid_path=x\nsystem_developer_keyid_path=y\n",
        ] {
            std::fs::write(&config, bad).unwrap();
            std::fs::remove_file(&record).unwrap();
            let output = Command::new("sh")
                .args(["-c", &script, "audb-rpm", &"a".repeat(64), "touch"])
                .arg(&record)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(42));
            assert!(!record.exists());
            // Recreate only the test marker for the next iteration.
            std::fs::write(&record, "").unwrap();
        }
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
                    .filter(|c| (c.contains("rpm -ivh") || c.contains("rpm -Uvh"))
                        && !c.contains("--test"))
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
            assert!(!t
                .calls
                .iter()
                .any(|c| c.contains("rpm -ivh") || c.contains("rpm -Uvh")));
            assert!(t.calls.last().unwrap().contains("rmdir"));
        }
    }
    /// The measured T1100 pair: rpm's family label is `arm` while `%{_target_cpu}`
    /// and every installed architecture-specific package are `armv7hl`.
    #[tokio::test]
    async fn armv7hl_device_accepts_armv7hl_package() {
        let mut t = Mock {
            metadata: Some("audb-agent\t0.3.0\t1\tarmv7hl\n".into()),
            arch: Some("armv7hl arm\n".into()),
            installed: Some("audb-agent\t0.3.0\t1\tarmv7hl\n".into()),
            ..Default::default()
        };
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
    }

    #[tokio::test]
    async fn cross_bitness_packages_are_rejected_before_root() {
        for (device, package) in [("aarch64", "armv7hl"), ("armv7hl arm", "aarch64")] {
            let mut t = Mock {
                metadata: Some(format!("audb-agent\t1\t1\t{package}\n")),
                arch: Some(device.into()),
                ..Default::default()
            };
            let error = install_bytes(
                &mut t,
                &[],
                &SystemPackageOptions::default(),
                Some(AGENT_PACKAGE_NAME),
            )
            .await
            .unwrap_err();
            assert_eq!(error.code, ErrorCode::InvalidArgument);
            assert!(!t
                .calls
                .iter()
                .any(|c| c.contains("rpm -ivh") || c.contains("rpm -Uvh")));
            assert!(t.calls.last().unwrap().contains("rmdir"));
        }
    }

    /// An rpm that cannot resolve `%{_target_cpu}` reports only the family label.
    /// The gate then refuses instead of inventing an `armv7hl` mapping, so an
    /// installable package fails closed with both observed values named. Today's
    /// Aurora rpm resolves `%{_target_cpu}` (armv7hl on t1100), so this path is
    /// reached only on devices that cannot name their package architecture.
    #[tokio::test]
    async fn unresolvable_target_cpu_refuses_rather_than_aliasing() {
        let mut t = Mock {
            metadata: Some("audb-agent\t1\t1\tarmv7hl\n".into()),
            arch: Some("%{_target_cpu} arm\n".into()),
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
        assert!(
            rpm_command("a'b.rpm", &SystemPackageOptions::default(), false)
                .ends_with("'a'\\''b.rpm'")
        );
    }
}
