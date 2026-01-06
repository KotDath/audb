use anyhow::{anyhow, Result};
use daemonize::Daemonize;
use directories::BaseDirs;
use std::fs;
use std::path::PathBuf;
use tracing::info;

/// Get the path to the server's PID file
pub fn pid_file_path() -> Result<PathBuf> {
    let base_dirs = BaseDirs::new().ok_or_else(|| anyhow!("Could not determine home directory"))?;
    let config_dir = base_dirs.config_dir().join("audb");
    fs::create_dir_all(&config_dir)?;
    Ok(config_dir.join("server.pid"))
}

/// Get the path to the server's log file
pub fn log_file_path() -> Result<PathBuf> {
    let base_dirs = BaseDirs::new().ok_or_else(|| anyhow!("Could not determine home directory"))?;
    let config_dir = base_dirs.config_dir().join("audb");
    fs::create_dir_all(&config_dir)?;
    Ok(config_dir.join("server.log"))
}

/// Daemonize the server process and run it in the background
pub fn daemonize_and_run() -> Result<()> {
    let pid_file = pid_file_path()?;
    let log_file = log_file_path()?;

    // Check if server is already running
    if is_server_running()? {
        return Err(anyhow!("Server is already running"));
    }

    // Open log file for stdout/stderr redirection
    let stdout = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)?;
    let stderr = stdout.try_clone()?;

    // Clone paths for logging after daemonization
    let pid_file_display = pid_file.display().to_string();
    let log_file_display = log_file.display().to_string();

    // Daemonize the process
    let daemonize = Daemonize::new()
        .pid_file(pid_file)
        .working_directory("/tmp")
        .stdout(stdout)
        .stderr(stderr);

    match daemonize.start() {
        Ok(_) => {
            // We're now in the daemon process
            // Initialize logging AFTER daemonizing (in the child process)
            tracing_subscriber::fmt()
                .with_target(false)
                .with_thread_ids(false)
                .with_ansi(false) // Disable ANSI colors in log file
                .init();

            info!("Server daemonized successfully");
            info!("PID file: {}", pid_file_display);
            info!("Log file: {}", log_file_display);

            // Create tokio runtime AFTER daemonizing
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;

            // Run the server
            runtime.block_on(crate::run_server())
        }
        Err(e) => Err(anyhow!("Failed to daemonize: {}", e)),
    }
}

/// Check if the server is already running based on PID file
pub fn is_server_running() -> Result<bool> {
    let pid_file = pid_file_path()?;

    if !pid_file.exists() {
        return Ok(false);
    }

    // Read PID from file
    let pid_str = fs::read_to_string(&pid_file)?;
    let pid: i32 = pid_str.trim().parse()
        .map_err(|_| anyhow!("Invalid PID in file"))?;

    // Check if process with that PID is running
    Ok(is_process_running(pid))
}

/// Check if a process with the given PID is running
#[cfg(unix)]
fn is_process_running(pid: i32) -> bool {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;

    // Send signal 0 (null signal) to check if process exists
    kill(Pid::from_raw(pid), Signal::SIGCONT).is_ok()
        || kill(Pid::from_raw(pid), None).is_ok()
}
