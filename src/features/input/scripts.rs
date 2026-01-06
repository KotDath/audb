use anyhow::Result;
use crate::tools::{session::DeviceSession, ssh::SshClient};
use russh::client::Handle;
use std::path::Path;

// Embed scripts at compile time
const TAP_SCRIPT: &str = include_str!("../../../scripts/tap.py");
const SWIPE_SCRIPT: &str = include_str!("../../../scripts/swipe.py");

const REMOTE_TAP_PATH: &str = "/tmp/audb_tap.py";
const REMOTE_SWIPE_PATH: &str = "/tmp/audb_swipe.py";

pub struct ScriptManager;

impl ScriptManager {
    /// Ensure tap script is present on device (using DeviceSession)
    pub fn ensure_tap_script_with_session(session: &mut DeviceSession) -> Result<()> {
        Self::ensure_script_with_session(session, REMOTE_TAP_PATH, TAP_SCRIPT)
    }

    /// Ensure swipe script is present on device (using DeviceSession)
    pub fn ensure_swipe_script_with_session(session: &mut DeviceSession) -> Result<()> {
        Self::ensure_script_with_session(session, REMOTE_SWIPE_PATH, SWIPE_SCRIPT)
    }

    /// Ensure script is present on device with correct content (using DeviceSession)
    fn ensure_script_with_session(
        session: &mut DeviceSession,
        remote_path: &str,
        script_content: &str,
    ) -> Result<()> {
        // Check if script exists with correct size
        let expected_size = script_content.len();
        let check_cmd = format!(
            "test -f {} && stat -c %s {} || echo 0",
            remote_path, remote_path
        );

        let result = session.exec(&check_cmd)?;
        let current_size: usize = result
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        if current_size != expected_size {
            // Upload needed
            let temp_file = std::env::temp_dir().join(
                Path::new(remote_path).file_name().unwrap()
            );
            std::fs::write(&temp_file, script_content)?;

            session.upload_file(&temp_file, Path::new(remote_path))?;

            // Make executable
            session.exec(&format!("chmod +x {}", remote_path))?;

            // Cleanup local temp
            std::fs::remove_file(&temp_file).ok();
        }

        Ok(())
    }

    // Legacy methods for backward compatibility - deprecated
    #[deprecated(since = "0.1.0", note = "Use ensure_tap_script_with_session instead")]
    pub fn ensure_tap_script(session: &mut Handle<SshClient>) -> Result<()> {
        Self::ensure_script(session, REMOTE_TAP_PATH, TAP_SCRIPT)
    }

    #[deprecated(since = "0.1.0", note = "Use ensure_swipe_script_with_session instead")]
    pub fn ensure_swipe_script(session: &mut Handle<SshClient>) -> Result<()> {
        Self::ensure_script(session, REMOTE_SWIPE_PATH, SWIPE_SCRIPT)
    }

    #[allow(deprecated)]
    fn ensure_script(
        session: &mut Handle<SshClient>,
        remote_path: &str,
        script_content: &str,
    ) -> Result<()> {
        // Check if script exists with correct size
        let expected_size = script_content.len();
        let check_cmd = format!(
            "test -f {} && stat -c %s {} || echo 0",
            remote_path, remote_path
        );

        let result = SshClient::exec(session, &check_cmd)?;
        let current_size: usize = result
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        if current_size != expected_size {
            // Upload needed
            let temp_file = std::env::temp_dir().join(
                Path::new(remote_path).file_name().unwrap()
            );
            std::fs::write(&temp_file, script_content)?;

            SshClient::upload(session, &temp_file, Path::new(remote_path))?;

            // Make executable
            SshClient::exec(session, &format!("chmod +x {}", remote_path))?;

            // Cleanup local temp
            std::fs::remove_file(&temp_file).ok();
        }

        Ok(())
    }

    pub fn tap_script_path() -> &'static str {
        REMOTE_TAP_PATH
    }

    pub fn swipe_script_path() -> &'static str {
        REMOTE_SWIPE_PATH
    }
}
