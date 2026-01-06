use audb_core::tools::types::Device;
use std::time::Instant;

/// Connection state for a device
#[derive(Debug, Clone)]
pub enum ConnectionState {
    Disconnected,
    Connecting {
        attempt: u32,
        #[allow(dead_code)]
        next_retry: Instant,
    },
    Connected {
        since: Instant,
    },
    #[allow(dead_code)]
    Errored {
        error: String,
        next_retry: Option<Instant>,
    },
    #[allow(dead_code)]
    Disabled,
}

/// Connection statistics
#[derive(Debug, Clone)]
pub struct ConnectionStats {
    pub connect_attempts: u64,
    pub successful_commands: u64,
    pub failed_commands: u64,
    pub last_error: Option<String>,
}

impl Default for ConnectionStats {
    fn default() -> Self {
        Self {
            connect_attempts: 0,
            successful_commands: 0,
            failed_commands: 0,
            last_error: None,
        }
    }
}

/// Wrapper around a device connection
#[derive(Clone)]
pub struct DeviceConnection {
    pub device: Device,
    pub state: ConnectionState,
    pub stats: ConnectionStats,
}

impl DeviceConnection {
    /// Create a new device connection in Disconnected state
    pub fn new(device: Device) -> Self {
        Self {
            device,
            state: ConnectionState::Disconnected,
            stats: ConnectionStats::default(),
        }
    }

    /// Get connection duration if connected
    #[allow(dead_code)]
    pub fn connection_duration(&self) -> Option<std::time::Duration> {
        match &self.state {
            ConnectionState::Connected { since } => Some(since.elapsed()),
            _ => None,
        }
    }
}
