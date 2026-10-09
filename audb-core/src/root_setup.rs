//! Explicit, one-time SSH provisioning. Passwords never enter the registry.
use crate::{
    devices::DeviceConfig,
    transport::{shell_quote, DeviceTransport},
    CoreError, CoreResult,
};
use audb_protocol::{ErrorCode, RootPassword};
use russh::keys::ssh_key::{rand_core::OsRng, Algorithm, LineEnding, PrivateKey};
use std::{fs, io::Write, path::Path};

pub async fn probe(device: &DeviceConfig) -> CoreResult<DeviceConfig> {
    let mut configured = device.clone();
    configured.root_user.get_or_insert_with(|| "root".into());
    let uid = DeviceTransport::new(configured.clone())
        .exec("id -u", true)
        .await?;
    if uid != "0" {
        return Err(CoreError::new(
            ErrorCode::RootAccessRequired,
            "Configured root SSH account is not UID 0",
        ));
    }
    Ok(configured)
}

/// Generate one identity per device and authorize it for the existing SSH user
/// and root. Keeping the registry's single identity preserves its v1 schema.
pub async fn configure(
    device: &DeviceConfig,
    password: &RootPassword,
    directory: &Path,
) -> CoreResult<DeviceConfig> {
    device.validate(false)?;
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::symlink_metadata(directory)?.file_type().is_symlink() {
            return Err(CoreError::invalid(
                "SSH identity directory must not be a symlink",
            ));
        }
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    let path = directory.join(format!("{}-root", device.id));
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(format!("{}-root.lock", device.id)))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    if !path.exists() {
        let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519)
            .map_err(|e| CoreError::runtime(format!("Cannot generate SSH identity: {e}")))?;
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        file.write_all(
            key.to_openssh(LineEnding::LF)
                .map_err(|e| CoreError::runtime(e.to_string()))?
                .as_bytes(),
        )?;
        file.as_file().sync_all()?;
        file.persist_noclobber(&path)
            .map_err(|e| CoreError::runtime(e.to_string()))?;
    }
    if fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Err(CoreError::invalid("SSH identity must not be a symlink"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    let key = PrivateKey::read_openssh_file(&path)
        .map_err(|e| CoreError::runtime(format!("Cannot load SSH identity: {e}")))?;
    let public = key
        .public_key()
        .to_openssh()
        .map_err(|e| CoreError::runtime(e.to_string()))?;
    let mut configured = device.clone();
    configured.ssh_key = Some(path);
    configured.root_user.get_or_insert_with(|| "root".into());
    configured.validate(true)?;
    let mut transport = DeviceTransport::new(device.clone());
    // Check credentials and UID before any remote mutation.
    transport.exec_privileged("true", Some(password)).await?;
    let root_user = configured.root_user.as_deref().unwrap();
    let root_added = transport
        .exec_privileged(&authorize(&public, Some(root_user), false), Some(password))
        .await?
        .lines()
        .any(|line| line == "AUDB_KEY_ADDED");
    let user_result = transport
        .exec(&authorize(&public, None, false), false)
        .await;
    let user_added = user_result
        .as_ref()
        .is_ok_and(|s| s.lines().any(|line| line == "AUDB_KEY_ADDED"));
    let user_uncertain = user_result
        .as_ref()
        .is_err_and(|error| error.code == ErrorCode::OutcomeUnknown);
    let verified = match user_result {
        Ok(_) => match probe(&configured).await {
            Ok(_) => DeviceTransport::new(configured.clone())
                .exec("true", false)
                .await
                .map(|_| ()),
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    if let Err(error) = verified {
        let mut cleanup_ok = !user_uncertain;
        if root_added {
            cleanup_ok &= transport
                .exec_privileged(&authorize(&public, Some(root_user), true), Some(password))
                .await
                .is_ok();
        }
        if user_added {
            cleanup_ok &= transport
                .exec(&authorize(&public, None, true), false)
                .await
                .is_ok();
        }
        return Err(CoreError::new(ErrorCode::RootAccessRequired,
            "Root SSH verification failed. Device SSH policy may prohibit root login or use a custom AuthorizedKeysFile; sshd configuration was not changed")
            .with_data(serde_json::json!({"verified":false,"registryChanged":false,"keyCleanup":cleanup_ok,"causeCode":error.code,"cause":error.message})));
    }
    Ok(configured)
}

fn authorize(public: &str, root: Option<&str>, remove: bool) -> String {
    let home = match root {
        Some(user) => format!("entry=$(getent passwd {}); test \"$(printf '%s' \"$entry\" | cut -d: -f3)\" = 0 || exit 1; home=$(printf '%s' \"$entry\" | cut -d: -f6);", shell_quote(user)),
        None => "home=$HOME;".into(),
    };
    let action = if remove {
        format!("temp=$(mktemp \"$dir/.audb-keys.XXXXXX\"); trap 'rm -f \"$temp\"' EXIT; grep -Fvx -- {key} \"$file\" > \"$temp\" || test $? = 1; chmod 600 \"$temp\"; mv \"$temp\" \"$file\";", key=shell_quote(public))
    } else {
        format!("if ! grep -Fqx -- {key} \"$file\"; then printf '\\n%s\\n' {key} >> \"$file\"; printf 'AUDB_KEY_ADDED\\n'; fi;", key=shell_quote(public))
    };
    format!("set -e; umask 077; {home} case \"$home\" in /*) ;; *) exit 1;; esac; dir=\"$home/.ssh\"; file=\"$dir/authorized_keys\"; test ! -L \"$dir\"; test ! -L \"$file\"; if test -e \"$file\"; then test -f \"$file\"; fi; mkdir -p \"$dir\"; chmod 700 \"$dir\"; touch \"$file\"; chmod 600 \"$file\"; {action}")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn run(home: &Path, key: &str, remove: bool) -> std::process::Output {
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(authorize(key, None, remove))
            .env("HOME", home)
            .output()
            .unwrap()
    }
    #[test]
    fn authorization_is_idempotent_and_rollback_preserves_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".ssh")).unwrap();
        let file = dir.path().join(".ssh/authorized_keys");
        fs::write(&file, "existing-key without trailing newline").unwrap();
        let key = "ssh-ed25519 fixture-public-key";
        assert!(run(dir.path(), key, false).status.success());
        let contents = fs::read(&file).unwrap();
        assert!(run(dir.path(), key, false).status.success());
        assert_eq!(fs::read(&file).unwrap(), contents);
        assert!(run(dir.path(), key, true).status.success());
        assert_eq!(
            fs::read_to_string(file).unwrap(),
            "existing-key without trailing newline\n"
        );
    }
    #[test]
    fn authorization_refuses_symlinked_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("unrelated");
        fs::write(&target, "preserve").unwrap();
        fs::create_dir(dir.path().join(".ssh")).unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join(".ssh/authorized_keys")).unwrap();
        assert!(!run(dir.path(), "ssh-ed25519 fixture", false)
            .status
            .success());
        assert_eq!(fs::read_to_string(target).unwrap(), "preserve");
    }
    #[test]
    fn authorization_refuses_symlinked_directories() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("unrelated");
        fs::create_dir(&target).unwrap();
        let file = target.join("authorized_keys");
        fs::write(&file, "preserve").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join(".ssh")).unwrap();
        assert!(!run(dir.path(), "ssh-ed25519 fixture", false)
            .status
            .success());
        assert_eq!(fs::read_to_string(file).unwrap(), "preserve");
    }
}
