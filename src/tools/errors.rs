/// Error types for audb operations
///
/// This module provides structured error types using thiserror for better error handling
/// and contextual error messages throughout the application.
use thiserror::Error;

/// Errors related to device operations
#[derive(Error, Debug)]
pub enum DeviceError {
    /// Device was not found in the device store
    #[error("Device not found: {0}")]
    NotFound(String),

    /// Root password is not configured for the device
    #[error("Root password not configured for device: {0}")]
    RootPasswordNotConfigured(String),

    /// Failed to connect to the device
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    /// SSH operation error
    #[error("SSH error: {0}")]
    SshError(#[from] anyhow::Error),

    /// Device validation error
    #[error("Device validation failed: {0}")]
    ValidationError(String),
}

/// Errors related to configuration file operations
#[derive(Error, Debug)]
pub enum ConfigError {
    /// Failed to read configuration file
    #[error("Failed to read config: {0}")]
    ReadError(#[from] std::io::Error),

    /// Failed to parse configuration file
    #[error("Invalid config format: {0}")]
    ParseError(#[from] serde_json::Error),

    /// Configuration validation failed
    #[error("Config validation failed: {0}")]
    ValidationError(String),

    /// Configuration file not found
    #[error("Config file not found at: {0}")]
    NotFound(String),
}

/// Errors related to input operations (tap, swipe, screenshot)
#[derive(Error, Debug)]
pub enum InputError {
    /// Invalid coordinates provided
    #[error("Invalid coordinates: {0}")]
    InvalidCoordinates(String),

    /// Script execution failed
    #[error("Script execution failed: {0}")]
    ScriptExecutionFailed(String),

    /// D-Bus command failed
    #[error("D-Bus command failed: {0}")]
    DbusCommandFailed(String),

    /// Device operation error
    #[error(transparent)]
    DeviceError(#[from] DeviceError),
}

/// Errors related to package installation
#[derive(Error, Debug)]
pub enum InstallError {
    /// RPM file is invalid or not found
    #[error("Invalid RPM file: {0}")]
    InvalidRpmFile(String),

    /// Package installation failed
    #[error("Package installation failed: {0}")]
    InstallationFailed(String),

    /// Device operation error
    #[error(transparent)]
    DeviceError(#[from] DeviceError),
}
