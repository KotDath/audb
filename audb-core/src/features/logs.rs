// Logs command implementation for Aurora OS devices
//
// Retrieve device logs via journalctl with Android/iOS-like interface.

use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::{
    macros::{print_error, print_info},
    session::DeviceSession,
    shell_escape::escape_single_quote,
    types::DeviceIdentifier,
};
use anyhow::{anyhow, Context, Result};

pub struct LogsArgs {
    pub lines: usize,
    pub priority: Option<crate::LogLevel>,
    pub unit: Option<String>,
    pub grep: Option<String>,
    pub since: Option<String>,
    pub clear: bool,
    pub force: bool,
    pub kernel: bool,
}

pub async fn execute(args: LogsArgs) -> Result<()> {
    if args.clear {
        return execute_clear_logs(args.force).await;
    }

    validate_args(&args)?;

    // Get device and establish session
    let current_host = DeviceState::get_current()?;
    let device_id = DeviceIdentifier::Host(current_host);
    let device = DeviceStore::find(&device_id)?;

    let mut session = DeviceSession::connect(&device)
        .context("Failed to connect to device")?;

    // Build and execute journalctl command
    let command = build_journalctl_command(&args)?;
    print_info(format!("Retrieving logs from {}...", device.display_name()));

    let output = session.exec_as_root(&command)
        .context("Failed to retrieve logs. Root access required.")?;

    // Print logs
    for line in &output {
        println!("{}", line);
    }

    if output.is_empty() {
        print_info("No logs found matching the criteria");
    }

    Ok(())
}

async fn execute_clear_logs(force: bool) -> Result<()> {
    // Require --force flag to prevent accidents
    if !force {
        return Err(anyhow!(
            "Clearing logs requires --force flag. Use: audb logs --clear --force"
        ));
    }

    // Get device and establish session
    let current_host = DeviceState::get_current()?;
    let device_id = DeviceIdentifier::Host(current_host);
    let device = DeviceStore::find(&device_id)?;

    let mut session = DeviceSession::connect(&device)
        .context("Failed to connect to device")?;

    print_info(format!(
        "Clearing logs on {}...",
        device.display_name()
    ));

    let command = "journalctl --rotate && journalctl --vacuum-time=1s";
    session
        .exec_as_root(command)
        .context("Failed to clear logs. Root access required.")?;

    print_info("Logs cleared successfully");
    Ok(())
}

fn validate_args(args: &LogsArgs) -> Result<()> {
    // Validate lines count
    if args.lines == 0 {
        return Err(anyhow!("Lines count must be greater than 0"));
    }
    if args.lines > 10000 {
        print_error("Large line count may take time to retrieve");
    }

    // Validate that kernel and unit are mutually exclusive
    if args.kernel && args.unit.is_some() {
        return Err(anyhow!(
            "Cannot specify both --kernel and --unit"
        ));
    }

    Ok(())
}

fn build_journalctl_command(args: &LogsArgs) -> Result<String> {
    let mut cmd = String::from("journalctl");

    // Kernel messages mode
    if args.kernel {
        cmd.push_str(" -k");
    }

    // Number of lines
    cmd.push_str(&format!(" -n {}", args.lines));

    // Priority level
    if let Some(ref priority) = args.priority {
        cmd.push_str(&format!(" -p {}", priority.to_journalctl_priority()));
    }

    // Unit filter (with shell escaping)
    if let Some(ref unit) = args.unit {
        let escaped = escape_single_quote(unit);
        cmd.push_str(&format!(" -u '{}'", escaped));
    }

    // Time filter (with shell escaping)
    if let Some(ref since) = args.since {
        let escaped = escape_single_quote(since);
        cmd.push_str(&format!(" --since '{}'", escaped));
    }

    // Output options
    cmd.push_str(" --no-pager --no-hostname");

    // Grep filter (as pipe, with escaping)
    if let Some(ref grep_pattern) = args.grep {
        let escaped = escape_single_quote(grep_pattern);
        cmd.push_str(&format!(" | grep '{}'", escaped));
    }

    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_basic_command() {
        let args = LogsArgs {
            lines: 100,
            priority: None,
            unit: None,
            since: None,
            grep: None,
            clear: false,
            force: false,
            kernel: false,
        };

        let cmd = build_journalctl_command(&args).unwrap();
        assert!(cmd.contains("journalctl"));
        assert!(cmd.contains("-n 100"));
        assert!(cmd.contains("--no-pager"));
        assert!(cmd.contains("--no-hostname"));
    }

    #[test]
    fn test_build_command_with_priority() {
        let args = LogsArgs {
            lines: 50,
            priority: Some(crate::LogLevel::Err),
            unit: None,
            since: None,
            grep: None,
            clear: false,
            force: false,
            kernel: false,
        };

        let cmd = build_journalctl_command(&args).unwrap();
        assert!(cmd.contains("-p err"));
    }

    #[test]
    fn test_build_command_with_unit() {
        let args = LogsArgs {
            lines: 100,
            priority: None,
            unit: Some("test.service".to_string()),
            since: None,
            grep: None,
            clear: false,
            force: false,
            kernel: false,
        };

        let cmd = build_journalctl_command(&args).unwrap();
        assert!(cmd.contains("-u 'test.service'"));
    }

    #[test]
    fn test_build_command_with_kernel() {
        let args = LogsArgs {
            lines: 100,
            priority: None,
            unit: None,
            since: None,
            grep: None,
            clear: false,
            force: false,
            kernel: true,
        };

        let cmd = build_journalctl_command(&args).unwrap();
        assert!(cmd.contains(" -k"));
    }

    #[test]
    fn test_build_command_with_grep() {
        let args = LogsArgs {
            lines: 100,
            priority: None,
            unit: None,
            since: None,
            grep: Some("ERROR".to_string()),
            clear: false,
            force: false,
            kernel: false,
        };

        let cmd = build_journalctl_command(&args).unwrap();
        assert!(cmd.contains("| grep 'ERROR'"));
    }

    #[test]
    fn test_shell_injection_protection() {
        let args = LogsArgs {
            lines: 100,
            priority: None,
            unit: Some("evil'; rm -rf /; echo '".to_string()),
            since: None,
            grep: None,
            clear: false,
            force: false,
            kernel: false,
        };

        let cmd = build_journalctl_command(&args).unwrap();
        // Should escape single quotes
        assert!(cmd.contains("'\\''"));
    }

    #[test]
    fn test_validate_kernel_unit_conflict() {
        let args = LogsArgs {
            lines: 100,
            priority: None,
            unit: Some("test.service".to_string()),
            since: None,
            grep: None,
            clear: false,
            force: false,
            kernel: true,
        };

        assert!(validate_args(&args).is_err());
    }

    #[test]
    fn test_validate_zero_lines() {
        let args = LogsArgs {
            lines: 0,
            priority: None,
            unit: None,
            since: None,
            grep: None,
            clear: false,
            force: false,
            kernel: false,
        };

        assert!(validate_args(&args).is_err());
    }

    #[test]
    fn test_validate_valid_args() {
        let args = LogsArgs {
            lines: 100,
            priority: None,
            unit: None,
            since: None,
            grep: None,
            clear: false,
            force: false,
            kernel: false,
        };

        assert!(validate_args(&args).is_ok());
    }
}
