use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::ssh::SshClient;
use anyhow::Result;
use std::sync::Arc;
use tokio::task::JoinSet;

pub async fn execute(active_only: bool) -> Result<()> {
    if active_only {
        list_active_devices().await
    } else {
        list_all_devices()
    }
}

fn list_all_devices() -> Result<()> {
    let devices = DeviceStore::list()?;

    if devices.is_empty() {
        println!("No devices configured. Use 'audb device add' to add a device.");
        return Ok(());
    }

    let current_host = DeviceState::get_current().ok();

    // Header
    println!("\x1b[1m{:<5} {:<20} {:<18} {:<6} {:<15} {:<10}\x1b[0m",
        "Index", "Name", "Host", "Port", "Platform", "Status");
    println!("{}", "-".repeat(80));

    for (idx, device) in devices.iter().enumerate() {
        let name = device.name.as_deref().unwrap_or("-");
        let status = if device.enabled {
            "\x1b[32menabled\x1b[0m"
        } else {
            "\x1b[90mdisabled\x1b[0m"
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
