/// DeviceSession abstraction for managing SSH connections to Aurora devices
///
/// This module provides a high-level interface for connecting to and executing
/// commands on Aurora OS devices, eliminating code duplication across features.

use crate::tools::{
    errors::DeviceError,
    ssh::SshClient,
    types::Device,
};
use anyhow::{Context, Result};
use russh::client::Handle;
use std::path::Path;

/// Manages an active SSH session to an Aurora device
///
/// This struct encapsulates the device connection and provides convenient
/// methods for executing commands, both as regular user and as root via devel-su.
///
/// # Example
/// ```no_run
/// use audb::tools::session::DeviceSession;
/// use audb::tools::types::Device;
///
/// # async fn example(device: Device) -> anyhow::Result<()> {
/// let mut session = DeviceSession::connect(&device).await?;
/// let output = session.exec("uname -a").await?;
/// println!("System info: {:?}", output);
/// # Ok(())
/// # }
/// ```
pub struct DeviceSession {
    device: Device,
    session: Handle<SshClient>,
}

impl DeviceSession {
    /// Connect to a device and return an active session
    ///
    /// # Arguments
    /// * `device` - The device configuration to connect to
    ///
    /// # Returns
    /// A new `DeviceSession` or an error if connection fails
    ///
    /// # Errors
    /// Returns `DeviceError::ConnectionFailed` if the SSH connection cannot be established
    pub fn connect(device: &Device) -> Result<Self, DeviceError> {
        let session = SshClient::connect(&device.host, device.port, &device.auth_path())
            .map_err(|e| DeviceError::ConnectionFailed(format!("{}", e)))?;

        Ok(Self {
            device: device.clone(),
            session,
        })
    }

    /// Execute command as regular user
    ///
    /// # Arguments
    /// * `command` - The shell command to execute
    ///
    /// # Returns
    /// A vector of output lines (stdout) from the command
    ///
    /// # Errors
    /// Returns an error if command execution fails or returns non-zero exit code
    pub fn exec(&mut self, command: &str) -> Result<Vec<String>> {
        SshClient::exec(&mut self.session, command)
            .with_context(|| format!("Failed to execute: {}", command))
    }

    /// Execute command as root via devel-su
    ///
    /// This method uses the device's configured root password to execute commands
    /// with root privileges using Aurora OS's devel-su mechanism.
    ///
    /// # Arguments
    /// * `command` - The shell command to execute as root
    ///
    /// # Returns
    /// A vector of output lines (stdout) from the command
    ///
    /// # Errors
    /// * Returns `DeviceError::RootPasswordNotConfigured` if no root password is set
    /// * Returns an error if command execution fails
    ///
    /// # Security
    /// The password and command are properly escaped to prevent shell injection.
    pub fn exec_as_root(&mut self, command: &str) -> Result<Vec<String>, DeviceError> {
        if self.device.root_password.is_empty() {
            return Err(DeviceError::RootPasswordNotConfigured(
                self.device.display_name(),
            ));
        }

        SshClient::exec_as_devel_su(&mut self.session, command, &self.device.root_password)
            .map_err(DeviceError::SshError)
            .with_context(|| format!("Failed to execute as root: {}", command))
            .map_err(DeviceError::SshError)
    }

    /// Upload file to device via SFTP
    ///
    /// # Arguments
    /// * `local_path` - Path to the local file to upload
    /// * `remote_path` - Destination path on the remote device
    ///
    /// # Errors
    /// Returns an error if file upload fails
    pub fn upload_file(&mut self, local_path: &Path, remote_path: &Path) -> Result<()> {
        SshClient::upload(&mut self.session, local_path, remote_path)
            .with_context(|| {
                format!(
                    "Failed to upload {} to {}",
                    local_path.display(),
                    remote_path.display()
                )
            })
    }

    /// Read remote file contents as base64 string
    ///
    /// This method is useful for reading files that may be owned by root,
    /// as it uses the root password if configured.
    ///
    /// # Arguments
    /// * `remote_path` - Path to the file on the remote device
    ///
    /// # Returns
    /// Base64-encoded contents of the file
    ///
    /// # Errors
    /// * Returns `DeviceError::RootPasswordNotConfigured` if no root password is set
    /// * Returns an error if file reading fails
    pub fn read_file_base64(&mut self, remote_path: &Path) -> Result<String, DeviceError> {
        if self.device.root_password.is_empty() {
            return Err(DeviceError::RootPasswordNotConfigured(
                self.device.display_name(),
            ));
        }

        SshClient::read_file_base64(&mut self.session, remote_path, &self.device.root_password)
            .map_err(DeviceError::SshError)
    }

    /// Get a reference to the underlying device configuration
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// Get the device's display name
    pub fn device_name(&self) -> String {
        self.device.display_name()
    }
}
