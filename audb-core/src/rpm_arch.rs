//! Device RPM platform architecture observation for installation gates.
//!
//! An RPM package is built for an architecture name, and rpm reports that name from
//! its platform configuration (`/etc/rpm/platform`, `%{_target_cpu}`) — not from
//! `%{_arch}`, which is a coarser family label. On an armv7hl Aurora device
//! `rpm --eval '%{_arch}'` reports `arm` while `%{_target_cpu}` and every one of the
//! installed architecture-specific packages report `armv7hl`, so comparing a package
//! against `%{_arch}` alone can never accept a real package.
//!
//! Both values are queried and both are accepted by exact match. That keeps the gate
//! non-regressive (whatever rpm reported before is still accepted) while adding the
//! value that actually names packages, and it introduces no alias: every accepted
//! value is a direct statement by the device's own rpm, and equality is per-token
//! exact, so no rule here can equate a 32-bit ARM name with `aarch64`.
use crate::{CoreError, CoreResult};

/// One remote command reporting the device's RPM platform architectures, in
/// preference order: `%{_target_cpu}` (the architecture rpm builds packages for)
/// then `%{_arch}` (rpm's family label).
///
/// rpm echoes macros it cannot resolve verbatim, so callers must not assume the
/// output is well formed; [`parse_platform_archs`] validates it.
pub(crate) const ARCH_QUERY: &str = "rpm --eval '%{_target_cpu} %{_arch}'";

/// RPM architecture names are `[A-Za-z0-9_]+` (for example `armv7hl`, `aarch64`,
/// `x86_64`, `noarch`). Anything else is an unresolved macro such as
/// `%{_target_cpu}`, a shell fragment from a compromised device, or noise.
pub(crate) fn usable_arch(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// `%{_libdir}` is an absolute install path such as `/usr/lib`; rpm echoes the
/// macro verbatim when it cannot resolve it.
pub(crate) fn usable_libdir(value: &str) -> bool {
    value.starts_with('/') && !value.contains('%') && !value.contains('{')
}

/// Parses the probe output into the deduplicated, validated architectures rpm
/// reports for this device, preserving their preference order.
pub(crate) fn parse_platform_archs(stdout: &str) -> CoreResult<Vec<String>> {
    let mut archs: Vec<String> = Vec::new();
    for token in stdout.split_whitespace() {
        if usable_arch(token) && !archs.iter().any(|known| known == token) {
            archs.push(token.to_owned());
        }
    }
    if archs.is_empty() {
        return Err(CoreError::runtime(
            "Device did not report an RPM architecture",
        ));
    }
    Ok(archs)
}

/// Accepts `noarch` or a package architecture that exactly matches one of the
/// architectures the device's rpm reports for itself.
///
/// This gate is deliberately narrower than rpm's `arch_compat` chain, which would
/// additionally accept e.g. `armv6hl` on an `armv7hl` device: it is an early,
/// clearly-worded pre-check, and the rpm transaction (or APM) remains the authority.
pub(crate) fn ensure_package_arch(package_arch: &str, platform_archs: &[String]) -> CoreResult<()> {
    if package_arch == "noarch" || platform_archs.iter().any(|arch| arch == package_arch) {
        return Ok(());
    }
    Err(CoreError::invalid(format!(
        "Package architecture '{package_arch}' does not match device architecture '{}'",
        platform_archs.join(" or ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_preference_order_and_drops_noise() {
        assert_eq!(
            parse_platform_archs("armv7hl arm\n").unwrap(),
            ["armv7hl", "arm"]
        );
        assert_eq!(parse_platform_archs("  aarch64\n\n").unwrap(), ["aarch64"]);
        assert_eq!(
            parse_platform_archs("aarch64 aarch64").unwrap(),
            ["aarch64"]
        );
        // rpm echoes unresolved macros verbatim.
        assert_eq!(parse_platform_archs("%{_target_cpu} arm").unwrap(), ["arm"]);
        // A compromised device cannot inject a shell fragment into the comparison.
        assert_eq!(parse_platform_archs("arm;id arm").unwrap(), ["arm"]);
        assert_eq!(
            parse_platform_archs("../../etc/passwd arm").unwrap(),
            ["arm"]
        );
    }

    #[test]
    fn rejects_output_without_a_usable_architecture() {
        for output in ["", "   \n", "%{_target_cpu} %{_arch}", "arm;id"] {
            let error = parse_platform_archs(output).unwrap_err();
            assert_eq!(error.code, audb_protocol::ErrorCode::RuntimeError);
        }
    }

    #[test]
    fn accepts_noarch_and_exact_platform_matches_only() {
        let armv7hl = parse_platform_archs("armv7hl arm").unwrap();
        let aarch64 = parse_platform_archs("aarch64").unwrap();
        for (platform, package) in [
            (&armv7hl, "armv7hl"), // the measured T1100 pair
            (&armv7hl, "arm"),     // accepted before this change; rpm decides
            (&armv7hl, "noarch"),
            (&aarch64, "aarch64"),
        ] {
            assert!(
                ensure_package_arch(package, platform).is_ok(),
                "{package} should be accepted on {platform:?}"
            );
        }
        for (platform, package) in [
            (&aarch64, "armv7hl"), // never treat 32-bit ARM as aarch64-compatible
            (&aarch64, "arm"),
            (&armv7hl, "aarch64"), // and never the other way round
            (&armv7hl, "armv6hl"), // narrower than rpm's arch_compat, deliberately
        ] {
            assert!(
                ensure_package_arch(package, platform).is_err(),
                "{package} should be rejected on {platform:?}"
            );
        }
    }

    #[test]
    fn mismatch_error_names_the_package_and_every_observed_architecture() {
        let platform = parse_platform_archs("armv7hl arm").unwrap();
        let error = ensure_package_arch("x86_64", &platform).unwrap_err();
        assert!(error.message.contains("'x86_64'"), "{}", error.message);
        assert!(
            error.message.contains("armv7hl or arm"),
            "{}",
            error.message
        );
    }

    /// The probe is a shell command; its behaviour cannot be proven by the remote
    /// transport mocks, which only pattern-match a command string. Run it against a
    /// stub `rpm` instead.
    #[cfg(unix)]
    #[test]
    fn probe_shell_command_reports_and_survives_broken_devices() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let run = |path: &str| {
            let output = Command::new("/bin/sh")
                .arg("-c")
                .arg(ARCH_QUERY)
                .env("PATH", path)
                .output()
                .unwrap();
            (
                String::from_utf8(output.stdout).unwrap(),
                output.status.success(),
            )
        };
        let stub = |body: &str| {
            let dir = tempfile::tempdir().unwrap();
            let rpm = dir.path().join("rpm");
            fs::write(&rpm, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&rpm, fs::Permissions::from_mode(0o755)).unwrap();
            dir
        };

        // A modern rpm resolves both macros.
        let resolving = stub("printf 'armv7hl arm\\n'");
        let (stdout, ok) = run(resolving.path().to_str().unwrap());
        assert!(ok, "stub rpm should succeed");
        assert_eq!(parse_platform_archs(&stdout).unwrap(), ["armv7hl", "arm"]);

        // An rpm that cannot resolve `%{_target_cpu}` still yields the family label.
        let unresolved = stub("printf '%%{_target_cpu} arm\\n'");
        let (stdout, ok) = run(unresolved.path().to_str().unwrap());
        assert!(ok);
        assert_eq!(parse_platform_archs(&stdout).unwrap(), ["arm"]);

        // No rpm at all: the caller must see a parse failure, not a fabricated arch.
        let empty = tempfile::tempdir().unwrap();
        let (stdout, ok) = run(empty.path().to_str().unwrap());
        assert!(!ok, "a missing rpm must fail the command");
        assert!(parse_platform_archs(&stdout).is_err());
    }
}
