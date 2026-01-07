use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::ssh::SshClient;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinSet;

pub async fn execute(active_only: bool) -> Result<()> {
    if active_only {
        list_active_devices().await
    } else {
        list_all_devices().await
    }
}

/// Try to get live status from server
async fn get_server_status() -> Option<HashMap<String, String>> {
    use std::path::PathBuf;
    use tokio::net::UnixStream;
    
    let uid = unsafe { libc::getuid() };
    let socket_path = PathBuf::from(format!("/tmp/audb-server-{}.sock", uid));
    
    if !socket_path.exists() {
        return None;
    }
    
    let mut stream = UnixStream::connect(&socket_path).await.ok()?;
    
    // Send ServerStatus command
    let request = audb_protocol::Request {
        id: 1,
        command: audb_protocol::Command::ServerStatus,
    };
    
    audb_protocol::send_message(&mut stream, &request).await.ok()?;
    let response: audb_protocol::Response = audb_protocol::recv_message(&mut stream).await.ok()?;
    
    if let audb_protocol::CommandResult::Success { output: audb_protocol::CommandOutput::Status(status) } = response.result {
        let mut map = HashMap::new();
        for device in status.devices {
            let state_str = match device.state {
                audb_protocol::ConnectionStateInfo::Disconnected => "disconnected".to_string(),
                audb_protocol::ConnectionStateInfo::Connecting { attempt } => format!("connecting({})", attempt),
                audb_protocol::ConnectionStateInfo::Connected { duration_secs } => format!("connected({}s)", duration_secs),
                audb_protocol::ConnectionStateInfo::Errored { ref error, .. } => {
                    // Shorten error message
                    let short_err = if error.len() > 20 { &error[..20] } else { error };
                    format!("error:{}", short_err)
                },
                audb_protocol::ConnectionStateInfo::Disabled => "disabled".to_string(),
            };
            map.insert(device.host, state_str);
        }
        Some(map)
    } else {
        None
    }
}

async fn list_all_devices() -> Result<()> {
    let devices = DeviceStore::list()?;

    if devices.is_empty() {
        println!("No devices configured. Use 'audb device add' to add a device.");
        return Ok(());
    }

    let current_host = DeviceState::get_current().ok();
    
    // Try to get live status from server
    let live_status = get_server_status().await;

    // Header
    println!("\x1b[1m{:<5} {:<20} {:<18} {:<6} {:<15} {:<10}\x1b[0m",
        "Index", "Name", "Host", "Port", "Platform", "Status");
    println!("{}", "-".repeat(80));

    for (idx, device) in devices.iter().enumerate() {
        let name = device.name.as_deref().unwrap_or("-");
        
        // Use live status if available, otherwise show config status
        let status = if let Some(ref live) = live_status {
            if let Some(state) = live.get(&device.host) {
                if state.starts_with("connected") {
                    format!("\x1b[32m{}\x1b[0m", state)
                } else if state.starts_with("error") || state == "disconnected" {
                    format!("\x1b[31m{}\x1b[0m", state)
                } else {
                    format!("\x1b[33m{}\x1b[0m", state)
                }
            } else if device.enabled {
                "\x1b[90mnot in server\x1b[0m".to_string()
            } else {
                "\x1b[90mdisabled\x1b[0m".to_string()
            }
        } else if device.enabled {
            "\x1b[32menabled\x1b[0m".to_string()
        } else {
            "\x1b[90mdisabled\x1b[0m".to_string()
        };

        let is_current = current_host.as_ref() == Some(&device.host);
        let marker = if is_current { " *" } else { "" };

        println!("{:<5} {:<20} {:<18} {:<6} {:<15} {}{}",
            idx,
            name,
            device.host,
            device.port,
            device.platform,
            status,
            marker
        );
    }

    if let Some(host) = current_host {
        println!("\n\x1b[36m*\x1b[0m Currently selected device: {}", host);
    }
    
    if live_status.is_some() {
        println!("\x1b[90m(live status from server)\x1b[0m");
    }

    Ok(())
}

async fn list_active_devices() -> Result<()> {
    let devices = DeviceStore::list_enabled()?;

    if devices.is_empty() {
        println!("No enabled devices configured.");
        return Ok(());
    }

    println!("Testing connections to {} device(s)...\n", devices.len());

    let current_host = DeviceState::get_current().ok();

    // Test connections concurrently
    let mut join_set = JoinSet::new();

    for (idx, device) in devices.iter().enumerate() {
        let device = Arc::new(device.clone());
        join_set.spawn(async move {
            let is_online = SshClient::test_connection(
                &device.host,
                device.port,
                &device.auth_path(),
            );
            (idx, device, is_online)
        });
    }

    let mut results = Vec::new();
    while let Some(result) = join_set.join_next().await {
        if let Ok(data) = result {
            results.push(data);
        }
    }

    // Sort by index to maintain order
    results.sort_by_key(|(idx, _, _)| *idx);

    // Display only active devices
    let active_results: Vec<_> = results.iter().filter(|(_, _, is_online)| *is_online).collect();

    if active_results.is_empty() {
        println!("No devices are currently reachable.");
        return Ok(());
    }

    // Header
    println!("\x1b[1m{:<5} {:<20} {:<18} {:<6} {:<15} {:<10}\x1b[0m",
        "Index", "Name", "Host", "Port", "Platform", "Status");
    println!("{}", "-".repeat(80));

    for (idx, device, _) in active_results {
        let name = device.name.as_deref().unwrap_or("-");
        let is_current = current_host.as_ref() == Some(&device.host);
        let marker = if is_current { " *" } else { "" };

        println!("{:<5} {:<20} {:<18} {:<6} {:<15} \x1b[32monline\x1b[0m{}",
            idx,
            name,
            device.host,
            device.port,
            device.platform,
            marker
        );
    }

    if let Some(host) = current_host {
        println!("\n\x1b[36m*\x1b[0m Currently selected device: {}", host);
    }

    Ok(())
}
