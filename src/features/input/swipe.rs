// Swipe command implementation for Aurora OS devices
//
// This feature requires root access to work properly. The Python script uses
// /dev/uinput which needs root permissions via devel-su.

use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::features::input::scripts::ScriptManager;
use crate::print_info;
use crate::tools::ssh::SshClient;
use crate::tools::types::DeviceIdentifier;
use anyhow::{anyhow, Result};

pub enum SwipeMode {
    Coords { x1: u16, y1: u16, x2: u16, y2: u16 },
    Direction(SwipeDirection),
}

#[derive(Debug)]
pub enum SwipeDirection {
    Left,
    Right,
    Up,
    Down,
}

impl SwipeDirection {
    pub fn to_script_arg(&self) -> &'static str {
        match self {
            SwipeDirection::Left => "rl",   // right-to-left
            SwipeDirection::Right => "lr",  // left-to-right
            SwipeDirection::Up => "du",     // down-to-up
            SwipeDirection::Down => "ud",   // up-to-down
        }
    }
}

pub async fn execute(mode: SwipeMode) -> Result<()> {
    // Validate coordinates if needed
    if let SwipeMode::Coords { x1, y1, x2, y2 } = &mode {
        for coord in [x1, y1, x2, y2] {
            if *coord > 4096 {
                return Err(anyhow!("Coordinate out of range: {}. Max: 4096", coord));
            }
        }
    }

    // Get device
    let current_host = DeviceState::get_current()?;
    let device_id = DeviceIdentifier::Host(current_host);
    let device = DeviceStore::find(&device_id)?;

    match &mode {
        SwipeMode::Coords { x1, y1, x2, y2 } => {
            print_info!("Swiping from ({},{}) to ({},{}) on device {}",
                x1, y1, x2, y2, device.display_name());
        }
        SwipeMode::Direction(dir) => {
            print_info!("Swiping {:?} on device {}", dir, device.display_name());
        }
    }

    // Connect as defaultuser
    print_info!("Connecting to {}:{}...", device.host, device.port);
    let mut session = SshClient::connect(&device.host, device.port, &device.auth_path())?;

    // Ensure swipe script is present on device
    ScriptManager::ensure_swipe_script(&mut session)?;

    // Build command
    let script_path = ScriptManager::swipe_script_path();
    let swipe_command = match mode {
        SwipeMode::Coords { x1, y1, x2, y2 } => {
            format!("python3 {} {} {} {} {}", script_path, x1, y1, x2, y2)
        }
        SwipeMode::Direction(dir) => {
            format!("python3 {} {}", script_path, dir.to_script_arg())
        }
    };

    // Execute swipe command using devel-su for root access
    print_info!("Executing swipe with devel-su...");
    match SshClient::exec_as_devel_su(&mut session, &swipe_command, &device.root_password) {
        Ok(output) => {
            // Display output
            for line in &output {
                if !line.is_empty() {
                    println!("{}", line);
                }
            }
            print_info!("Swipe completed successfully");
            Ok(())
        }
        Err(e) => {
            // Check if error is related to missing root password
            if e.to_string().contains("Root password not configured") {
                Err(anyhow!(
                    "Swipe requires root access. {}. \
                    Set root password using: audb device add",
                    e
                ))
            } else {
                Err(e)
            }
        }
    }
}
