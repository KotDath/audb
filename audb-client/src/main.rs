mod daemon;
mod package;

use audb_core::{
    devices::{DeviceConfig, DeviceKind, DeviceRegistry, EmulatorOptions},
    emulator, setup, EmulatorConfig,
};
use audb_protocol::{
    AudbError, Command, CommandOutput, ErrorCode, LogsOptions, PermissionAction, RootPassword,
    SwipeOptions, SystemPackageOptions, TrackPosition,
};
use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "audb",
    version,
    about = "Aurora Debug Bridge — device and emulator automation"
)]
struct Cli {
    #[arg(long, global = true)]
    socket: Option<String>,
    #[arg(short = 'd', long, global = true)]
    device: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    /// Maximum command duration, including queue wait, in seconds.
    #[arg(long, global = true, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=86400))]
    command_timeout: u64,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum PermissionCommand {
    /// Read declared permissions, saved grants and dialog state.
    List { application_id: String },
    /// Add declared permissions; existing grants are preserved.
    Grant {
        application_id: String,
        #[arg(
            required_unless_present = "all_requested",
            conflicts_with = "all_requested"
        )]
        permissions: Vec<String>,
        #[arg(long)]
        all_requested: bool,
        /// Allow turning off the dialog before granting (required when enabled).
        #[arg(long)]
        disable_prompt: bool,
    },
    /// Remove selected saved grants; does not change system policy.
    Revoke {
        application_id: String,
        #[arg(required = true)]
        permissions: Vec<String>,
    },
    /// Clear saved grants and enable the dialog without clearing application data.
    Reset { application_id: String },
    /// Enable (clears grants) or disable the dialog (preserves saved grants).
    Prompt {
        application_id: String,
        #[arg(long, required_unless_present = "disable", conflicts_with = "disable")]
        enable: bool,
        #[arg(long)]
        disable: bool,
    },
}
fn map_permission(command: PermissionCommand) -> Command {
    let (application_id, action) = match command {
        PermissionCommand::List { application_id } => (application_id, PermissionAction::List),
        PermissionCommand::Grant {
            application_id,
            permissions,
            all_requested,
            disable_prompt,
        } => (
            application_id,
            PermissionAction::Grant {
                permissions,
                all_requested,
                disable_prompt,
            },
        ),
        PermissionCommand::Revoke {
            application_id,
            permissions,
        } => (application_id, PermissionAction::Revoke { permissions }),
        PermissionCommand::Reset { application_id } => (application_id, PermissionAction::Reset),
        PermissionCommand::Prompt {
            application_id,
            enable,
            ..
        } => (application_id, PermissionAction::Prompt { enabled: enable }),
    };
    Command::Permission {
        application_id,
        action,
    }
}

#[derive(Subcommand)]
enum Commands {
    #[command(name = "__daemon", hide = true)]
    Daemon,
    #[command(name = "__shutdown", hide = true)]
    Shutdown,
    Tap {
        x: i32,
        y: i32,
        #[arg(long, default_value_t = 150)]
        duration: u64,
    },
    Swipe {
        #[arg(required = true)]
        args: Vec<String>,
        #[arg(long)]
        steps: Option<u32>,
        #[arg(long)]
        duration: Option<u64>,
        #[arg(long)]
        hold: Option<u64>,
    },
    Text {
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        string: Option<String>,
        /// Read exact UTF-8 text from stdin, including a trailing newline.
        #[arg(long, conflicts_with = "string")]
        stdin: bool,
        /// Delay between code points; 0 commits the whole string at once.
        #[arg(long, default_value_t = 0)]
        delay: u64,
    },
    Key {
        name: String,
    },
    Screenshot {
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    Permission {
        #[command(subcommand)]
        command: PermissionCommand,
    },
    Status,
    /// Read connection and component checks, with suggested fixes.
    Doctor,
    /// Read operation support, current readiness and limitations.
    Capabilities,
    Install,
    Uninstall,
    SetupStatus,
    /// Install the audb-agent system RPM on the selected device.
    SetupDevice(SetupDeviceArgs),
    Emulator {
        #[command(subcommand)]
        command: EmulatorCommand,
    },
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    Select {
        id: String,
    },
    Info {
        category: Option<String>,
    },
    Shell(ShellArgs),
    Launch {
        package: String,
    },
    Stop {
        package: String,
    },
    App {
        #[command(subcommand)]
        command: AppCommand,
    },
    Display {
        #[command(subcommand)]
        command: DisplayCommand,
    },
    Perf {
        #[command(subcommand)]
        command: PerfCommand,
    },
    Crash {
        #[command(subcommand)]
        command: CrashCommand,
    },
    Sandbox {
        #[command(subcommand)]
        command: SandboxCommand,
    },
    Network {
        #[command(subcommand)]
        command: NetworkCommand,
    },
    Location {
        #[command(subcommand)]
        command: LocationCommand,
    },
    Sensor {
        #[command(subcommand)]
        command: SensorCommand,
    },
    Clipboard {
        #[command(subcommand)]
        command: ClipboardCommand,
    },
    Logs(LogsArgs),
    Open {
        url: String,
    },
    Push {
        local: String,
        remote: String,
    },
    Pull {
        remote: String,
        local: Option<PathBuf>,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    Package {
        #[command(subcommand)]
        command: PackageCommand,
    },
}

#[derive(Subcommand)]
enum EmulatorCommand {
    Start {
        #[arg(long, default_value_t = 90)]
        timeout: u64,
    },
    Stop,
    Status,
}

#[derive(Subcommand)]
enum DeviceCommand {
    List,
    Current,
    Add(DeviceAddArgs),
    Update(DeviceUpdateArgs),
    Remove { id: String },
}

#[derive(Args)]
struct DeviceAddArgs {
    /// SSH destination: user@host or SSH profile name.
    #[arg(conflicts_with = "host", required_unless_present = "host")]
    destination: Option<String>,
    /// Stable device ID, independent of its address.
    #[arg(long)]
    id: String,
    /// Display name; defaults to the device ID.
    #[arg(long)]
    name: Option<String>,
    /// IP address, hostname or OpenSSH profile name.
    #[arg(long, required_unless_present = "destination")]
    host: Option<String>,
    /// SSH port; physical devices default to their SSH profile's port.
    #[arg(long)]
    port: Option<u16>,
    /// Private key file; omitted physical keys use OpenSSH profiles/SSH agent.
    #[arg(long)]
    key: Option<PathBuf>,
    /// SSH account; defaults to defaultuser for IP addresses.
    #[arg(long)]
    user: Option<String>,
    #[arg(long, default_value = "physical", value_parser = ["physical", "emulator"])]
    kind: String,
    /// Optional account for direct root SSH; no devel-su password is stored.
    #[arg(long)]
    root_user: Option<String>,
    /// QMP Unix socket path (emulator only).
    #[arg(long)]
    qmp: Option<PathBuf>,
    /// Aurora SDK directory (emulator only).
    #[arg(long)]
    sdk_root: Option<PathBuf>,
    /// SDK virtual machine name (emulator only).
    #[arg(long)]
    emulator_name: Option<String>,
}
#[derive(Args)]
struct DeviceUpdateArgs {
    id: String,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    user: Option<String>,
    #[arg(long, conflicts_with = "ssh_config")]
    key: Option<PathBuf>,
    /// Use OpenSSH profile/agent identities instead of an explicit key (physical only).
    #[arg(long)]
    ssh_config: bool,
    #[arg(long)]
    root_user: Option<String>,
    #[arg(long)]
    qmp: Option<PathBuf>,
    /// Aurora SDK directory (emulator only).
    #[arg(long)]
    sdk_root: Option<PathBuf>,
    /// SDK virtual machine name (emulator only).
    #[arg(long)]
    emulator_name: Option<String>,
}
impl DeviceAddArgs {
    fn into_config(self) -> Result<DeviceConfig, AudbError> {
        let (destination_user, host) = if let Some(destination) = self.destination {
            match destination.rsplit_once('@') {
                Some((user, host)) => (Some(user.to_owned()), host.to_owned()),
                None => (None, destination),
            }
        } else {
            (None, self.host.unwrap_or_default())
        };
        let kind = if self.kind == "emulator" {
            DeviceKind::Emulator
        } else {
            DeviceKind::Physical
        };
        let defaults = EmulatorConfig::default();
        let explicit_user = self.user.or(destination_user);
        let infer_profile_user =
            explicit_user.is_none() && host.parse::<std::net::IpAddr>().is_err();
        let mut d = DeviceConfig {
            name: self.name.unwrap_or_else(|| self.id.clone()),
            id: self.id,
            kind,
            host,
            ssh_port: self.port.unwrap_or(22),
            ssh_user: explicit_user.unwrap_or_else(|| "defaultuser".into()),
            ssh_key: self.key.map(absolute),
            root_user: self.root_user,
            emulator: None,
        };
        if kind == DeviceKind::Emulator {
            d.ssh_key = d.ssh_key.or(Some(defaults.ssh_key));
            d.root_user = d.root_user.or(Some(defaults.root_user));
            d.emulator = Some(EmulatorOptions {
                qmp_socket: self.qmp.map(absolute).unwrap_or(defaults.qmp_socket),
                sdk_root: self.sdk_root.map(absolute).unwrap_or(defaults.sdk_root),
                emulator_name: self.emulator_name.unwrap_or(defaults.emulator_name),
            });
        } else {
            if self.qmp.is_some() || self.sdk_root.is_some() || self.emulator_name.is_some() {
                return Err(error(
                    ErrorCode::InvalidArgument,
                    "QMP and SDK options require --kind emulator",
                ));
            }
            d.validate(false).map_err(core_error)?;
            // ssh -G reads local profiles only. No network connection or host trust change.
            let mut ssh = std::process::Command::new("ssh");
            ssh.arg("-G");
            if !infer_profile_user {
                ssh.args(["-o", &format!("User={}", d.ssh_user)]);
            }
            ssh.args(["--", &d.host]);
            let output = ssh.output().map_err(internal)?;
            if !output.status.success() {
                return Err(error(
                    ErrorCode::SshError,
                    String::from_utf8_lossy(&output.stderr),
                ));
            }
            if self.port.is_none() {
                if let Some(port) = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .find_map(|line| line.strip_prefix("port "))
                {
                    d.ssh_port = port.parse().map_err(internal)?;
                }
            }
            if infer_profile_user {
                if let Some(user) = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .find_map(|line| line.strip_prefix("user "))
                {
                    d.ssh_user = user.to_owned();
                }
            }
        }
        d.validate(true).map_err(core_error)?;
        Ok(d)
    }
}

#[derive(Args)]
struct ShellArgs {
    #[arg(long)]
    root: bool,
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    cmdline: Vec<String>,
}

#[derive(Subcommand)]
enum AppCommand {
    Launch {
        package: String,
    },
    Stop {
        package: String,
    },
    ListRunning,
    Pid {
        package: String,
    },
    WaitRunning(WaitArgs),
    WaitStopped(WaitArgs),
    ClearData {
        package: String,
        #[arg(long, conflicts_with = "confirm", required_unless_present = "confirm")]
        dry_run: bool,
        #[arg(long, conflicts_with = "dry_run", required_unless_present = "dry_run")]
        confirm: bool,
    },
}
#[derive(Args)]
struct WaitArgs {
    package: String,
    #[arg(long, default_value_t = 15.0)]
    timeout: f64,
    #[arg(long, default_value_t = 0.25)]
    interval: f64,
}

#[derive(Subcommand)]
enum DisplayCommand {
    Status,
    On(Timeout5),
    Off(Timeout5),
    Dim(Timeout5),
    Lock(Timeout5),
    Wake(Timeout5),
}
#[derive(Args)]
struct Timeout5 {
    #[arg(long, default_value_t = 5.0)]
    timeout: f64,
}

#[derive(Subcommand)]
enum PerfCommand {
    Snapshot {
        package: String,
        #[arg(long, default_value_t = 0.2)]
        sample_interval: f64,
    },
    Monitor {
        package: String,
        #[arg(long, default_value_t = 10.0)]
        duration: f64,
        #[arg(long, default_value_t = 0.5)]
        interval: f64,
    },
    VisualFps {
        #[arg(long, default_value_t = 5.0)]
        duration: f64,
        #[arg(long, default_value_t = 0.2)]
        interval: f64,
        #[arg(long, default_value_t = 1.0)]
        freeze_threshold: f64,
    },
}

#[derive(Subcommand)]
enum CrashCommand {
    List {
        package: Option<String>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long, default_value_t = 2000)]
        lines: usize,
    },
    Watch {
        package: String,
        #[arg(long, default_value_t = 30.0)]
        timeout: f64,
        #[arg(long, default_value_t = 0.5)]
        interval: f64,
    },
    Clear {
        package: Option<String>,
    },
}

#[derive(Subcommand)]
enum SandboxCommand {
    Paths {
        package: String,
    },
    List {
        package: String,
        kind: String,
        #[arg(default_value = "")]
        path: String,
    },
    Pull {
        package: String,
        kind: String,
        path: String,
        output: PathBuf,
    },
    Sqlite {
        package: String,
        kind: String,
        path: String,
        query: String,
    },
}

#[derive(Subcommand)]
enum NetworkCommand {
    Status,
    Interfaces,
    Traffic,
    Proxy {
        #[command(subcommand)]
        command: ProxyCommand,
    },
    Offline {
        state: String,
    },
}
#[derive(Subcommand)]
enum ProxyCommand {
    Get,
    Set { host: String, port: u16 },
    Clear,
}

#[derive(Subcommand)]
enum LocationCommand {
    Set {
        #[arg(allow_hyphen_values = true)]
        latitude: f64,
        #[arg(allow_hyphen_values = true)]
        longitude: f64,
        #[arg(default_value_t = 0.0, allow_hyphen_values = true)]
        altitude: f64,
    },
    Track {
        action: String,
        #[arg(allow_hyphen_values = true)]
        value: Option<String>,
        #[arg(long = "loop")]
        looped: Option<String>,
        #[arg(long)]
        speed: Option<i32>,
        #[arg(long)]
        default_interval: Option<String>,
    },
}

#[derive(Subcommand)]
enum SensorCommand {
    List,
    Enable {
        sensor: String,
    },
    Disable {
        sensor: String,
    },
    SetVector {
        sensor: String,
        #[arg(allow_hyphen_values = true)]
        x: f64,
        #[arg(allow_hyphen_values = true)]
        y: f64,
        #[arg(allow_hyphen_values = true)]
        z: f64,
    },
    SetScalar {
        sensor: String,
        #[arg(allow_hyphen_values = true)]
        value: i32,
    },
}
#[derive(Subcommand)]
enum ClipboardCommand {
    Status,
    Get,
    Set { text: String },
    Clear,
}

#[derive(Args)]
struct LogsArgs {
    #[arg(short = 'n', long, default_value_t = 100)]
    lines: usize,
    #[arg(short = 'p', long)]
    priority: Option<String>,
    #[arg(short = 'u', long)]
    unit: Option<String>,
    #[arg(long)]
    since: Option<String>,
    #[arg(long)]
    grep: Option<String>,
    #[arg(short = 'k', long)]
    kernel: bool,
    #[arg(long)]
    clear: bool,
    #[arg(long)]
    force: bool,
}

#[derive(Subcommand)]
enum PackageCommand {
    List {
        #[arg(long)]
        filter: Option<String>,
    },
    Install {
        rpm: String,
        /// Maximum wait for APM to register the requested version, in seconds.
        #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
    },
    /// Install a system RPM as root with a per-transaction validation override.
    InstallSystem(SystemInstallArgs),
    Uninstall {
        name: String,
    },
    Sign {
        rpm: String,
        #[arg(long)]
        key: Option<String>,
        #[arg(long)]
        cert: Option<String>,
    },
    Validate {
        rpm: String,
    },
}

#[derive(Args)]
struct SystemInstallArgs {
    rpm: PathBuf,
    #[command(flatten)]
    options: SystemInstallFlags,
}
#[derive(Args)]
struct SetupDeviceArgs {
    /// Path to audb-agent RPM; defaults to packages/audb-agent.rpm beside audb.
    #[arg(long)]
    rpm: Option<PathBuf>,
    #[command(flatten)]
    options: SystemInstallFlags,
}
#[derive(Args)]
struct SystemInstallFlags {
    /// Use rpm -Uvh instead of -ivh. Does not force replacement of the same version.
    #[arg(long)]
    upgrade: bool,
    /// Allow replacing the same package version via rpm --replacepkgs.
    #[arg(long)]
    reinstall: bool,
    /// Run only rpm --test; do not install the package.
    #[arg(long)]
    check_only: bool,
    /// Read one root password line from stdin for devel-su; never store it.
    #[arg(long)]
    root_password_stdin: bool,
}

fn system_install_request(
    path: PathBuf,
    flags: SystemInstallFlags,
    device: &DeviceConfig,
    setup: bool,
) -> Result<Command, AudbError> {
    use std::io::{BufRead, IsTerminal, Read};
    let path = std::fs::canonicalize(&path).map_err(|e| {
        error(
            ErrorCode::NotFound,
            format!(
                "RPM not found: {}: {e}. Supply --rpm for setup-device",
                path.display()
            ),
        )
    })?;
    if path.extension().and_then(|s| s.to_str()) != Some("rpm") {
        return Err(error(ErrorCode::InvalidArgument, "File must be .rpm"));
    }
    let mut magic = [0u8; 4];
    std::fs::File::open(&path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .map_err(internal)?;
    if magic != [0xed, 0xab, 0xee, 0xdb] {
        return Err(error(
            ErrorCode::InvalidArgument,
            "File does not have an RPM lead header",
        ));
    }
    let password = if flags.root_password_stdin {
        let mut bytes = Vec::new();
        std::io::stdin()
            .lock()
            .take(4099)
            .read_until(b'\n', &mut bytes)
            .map_err(internal)?;
        if bytes.len() > 4098 {
            return Err(error(
                ErrorCode::InvalidArgument,
                "Root credential is too long",
            ));
        }
        let mut value = String::from_utf8(bytes)
            .map_err(|_| error(ErrorCode::InvalidArgument, "Root credential must be UTF-8"))?;
        if value.ends_with('\n') {
            value.pop();
        }
        if value.ends_with('\r') {
            value.pop();
        }
        Some(RootPassword::new(value))
    } else if device.root_user.is_none() && device.ssh_user != "root" {
        if !std::io::stdin().is_terminal() {
            return Err(error(ErrorCode::RootAccessRequired,
                "Root installation needs --root-password-stdin, an interactive terminal, or device update ID --root-user root"));
        }
        Some(RootPassword::new(
            rpassword::prompt_password(format!("Root password for {}: ", device.id))
                .map_err(internal)?,
        ))
    } else {
        None
    };
    if let Some(password) = &password {
        let value = password.expose();
        if value.is_empty() || value.len() > 4096 || value.contains(['\n', '\r', '\0']) {
            return Err(error(
                ErrorCode::InvalidArgument,
                "Root credential must be one nonempty line",
            ));
        }
    }
    let options = SystemPackageOptions {
        upgrade: flags.upgrade,
        reinstall: flags.reinstall,
        check_only: flags.check_only,
        root_password: password,
    };
    let local_path = path
        .to_str()
        .ok_or_else(|| error(ErrorCode::InvalidArgument, "RPM path must be UTF-8"))?
        .to_owned();
    Ok(if setup {
        Command::SetupDevice {
            local_path,
            options,
        }
    } else {
        Command::SystemPackageInstall {
            local_path,
            options,
        }
    })
}

#[tokio::main]
async fn main() {
    let json_requested = std::env::args_os().any(|value| value == "--json");
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return;
        }
        Err(parse_error) if json_requested => {
            emit_error(
                true,
                None,
                &error(ErrorCode::InvalidArgument, parse_error.to_string()),
            );
            std::process::exit(1);
        }
        Err(parse_error) => parse_error.exit(),
    };
    let json_mode = cli.json;
    let mut target = None;
    match run(cli, &mut target).await {
        Ok(()) => {}
        Err(error) => {
            emit_error(json_mode, target.as_deref(), &error);
            std::process::exit(exit_code(error.code));
        }
    }
}

async fn run(cli: Cli, target: &mut Option<String>) -> Result<(), AudbError> {
    match cli.command {
        Commands::Daemon => return daemon::run().await.map_err(internal),
        Commands::Shutdown => {
            let output = daemon::typed_request(None, Command::Shutdown, 5_000).await?;
            emit(cli.json, None, output_to_value(output), None);
            return Ok(());
        }
        Commands::Device { command } => return device_command(cli.json, command, target),
        Commands::Select { id } => {
            DeviceRegistry::transaction(|r| {
                r.get(&id)?;
                r.default_device = Some(id.clone());
                Ok(())
            })
            .map_err(core_error)?;
            *target = Some(id.clone());
            return emit_local(cli.json, Some(&id), json!({"id":id,"selected":true}));
        }
        _ => {}
    }
    let registry = DeviceRegistry::load().map_err(core_error)?;
    let device = registry
        .resolve(cli.device.as_deref())
        .map_err(core_error)?;
    *target = Some(device.id.clone());
    let device_id = Some(device.id.as_str());
    // Host package tooling still uses the SDK on physical targets.
    let mut config = device.emulator_config().unwrap_or_default();
    if let Some(socket) = &cli.socket {
        config.qmp_socket = socket.into();
    }
    match cli.command {
        Commands::Install
        | Commands::Uninstall
        | Commands::SetupStatus
        | Commands::Emulator { .. } => {
            device.emulator_config().map_err(core_error)?;
        }
        _ => {}
    }
    match cli.command {
        Commands::Install => {
            return emit_local(
                cli.json,
                device_id,
                setup::install(&config).map_err(core_error)?,
            )
        }
        Commands::Uninstall => {
            return emit_local(
                cli.json,
                device_id,
                setup::uninstall(&config).map_err(core_error)?,
            )
        }
        Commands::SetupStatus => {
            return emit_local(
                cli.json,
                device_id,
                setup::status(&config).map_err(core_error)?,
            )
        }
        Commands::Emulator { command } => {
            let value = match command {
                EmulatorCommand::Start { timeout } => {
                    emulator::start(&config, Duration::from_secs(timeout))
                        .await
                        .map_err(core_error)?
                }
                EmulatorCommand::Stop => emulator::stop(&config, Duration::from_secs(30))
                    .await
                    .map_err(core_error)?,
                EmulatorCommand::Status => emulator::status(&config).await,
            };
            return emit_local(cli.json, device_id, value);
        }
        _ => {}
    }

    let (command, binary_output): (Command, Option<PathBuf>) = match cli.command {
        Commands::SetupDevice(args) => {
            let rpm = match args.rpm {
                Some(path) => path,
                None => std::env::current_exe()
                    .map_err(internal)?
                    .parent()
                    .ok_or_else(|| internal("Cannot determine executable directory"))?
                    .join("packages/audb-agent.rpm"),
            };
            (
                system_install_request(rpm, args.options, device, true)?,
                None,
            )
        }
        Commands::Tap { x, y, duration } => (
            Command::Tap {
                x,
                y,
                duration_ms: duration,
                socket: cli.socket,
            },
            None,
        ),
        Commands::Swipe {
            args,
            steps,
            duration,
            hold,
        } => (
            Command::Swipe {
                args,
                options: SwipeOptions {
                    steps,
                    duration_ms: duration,
                    hold_ms: hold,
                },
                socket: cli.socket,
            },
            None,
        ),
        Commands::Text {
            string,
            stdin,
            delay,
        } => {
            let text = if stdin {
                use std::io::Read;
                let mut value = String::new();
                std::io::stdin()
                    .take((audb_protocol::input::MAX_TEXT_BYTES + 1) as u64)
                    .read_to_string(&mut value)
                    .map_err(|e| {
                        error(
                            ErrorCode::InvalidArgument,
                            format!("Cannot read UTF-8 text: {e}"),
                        )
                    })?;
                value
            } else {
                string.unwrap_or_default()
            };
            audb_protocol::input::validate_text(&text, delay)
                .map_err(|e| error(ErrorCode::InvalidArgument, e))?;
            (
                Command::Text {
                    text,
                    delay_ms: delay,
                    socket: cli.socket,
                },
                None,
            )
        }
        Commands::Key { name } => (
            Command::Key {
                name,
                socket: cli.socket,
            },
            None,
        ),
        Commands::Screenshot { output } => {
            if cli.json && output.is_none() {
                return Err(error(
                    ErrorCode::InvalidArgument,
                    "--json screenshot requires --output",
                ));
            }
            (Command::Screenshot { socket: cli.socket }, output)
        }
        Commands::Status => (
            if device.kind == DeviceKind::Emulator {
                Command::QmpStatus { socket: cli.socket }
            } else {
                Command::DeviceStatus
            },
            None,
        ),
        Commands::Permission { command } => (map_permission(command), None),
        Commands::Doctor => (Command::Doctor, None),
        Commands::Capabilities => (Command::Capabilities, None),
        Commands::Info { category } => (Command::Info { category }, None),
        Commands::Shell(args) => (
            Command::Shell {
                root: args.root,
                command_line: args.cmdline.join(" "),
            },
            None,
        ),
        Commands::Launch { package } => (Command::AppLaunch { package }, None),
        Commands::Stop { package } => (Command::AppStop { package }, None),
        Commands::App { command } => (map_app(command), None),
        Commands::Display { command } => (map_display(command), None),
        Commands::Perf { command } => (map_perf(command, cli.socket), None),
        Commands::Crash { command } => (map_crash(command), None),
        Commands::Sandbox { command } => map_sandbox(command),
        Commands::Network { command } => (map_network(command)?, None),
        Commands::Location { command } => (map_location(command)?, None),
        Commands::Sensor { command } => (map_sensor(command), None),
        Commands::Clipboard { command } => (
            match command {
                ClipboardCommand::Status => Command::ClipboardStatus,
                _ => Command::ClipboardUnavailable,
            },
            None,
        ),
        Commands::Logs(args) => (
            Command::Logs {
                options: LogsOptions {
                    lines: args.lines,
                    priority: args.priority,
                    unit: args.unit,
                    since: args.since,
                    grep: args.grep,
                    kernel: args.kernel,
                    clear: args.clear,
                    force: args.force,
                },
            },
            None,
        ),
        Commands::Open { url } => (Command::Open { url }, None),
        Commands::Push { local, remote } => (
            Command::Push {
                local_path: local,
                remote_path: remote,
            },
            None,
        ),
        Commands::Pull {
            remote,
            local,
            output,
        } => (
            Command::Pull {
                remote_path: remote.clone(),
            },
            Some(output.or(local).unwrap_or_else(|| {
                PathBuf::from(PathBuf::from(remote).file_name().unwrap_or_default())
            })),
        ),
        Commands::Package { command } => match command {
            PackageCommand::InstallSystem(args) => (
                system_install_request(args.rpm, args.options, device, false)?,
                None,
            ),
            PackageCommand::List { filter } => (Command::PackageList { filter }, None),
            PackageCommand::Install { rpm, timeout } => {
                let path = std::fs::canonicalize(&rpm).map_err(|e| {
                    error(ErrorCode::NotFound, format!("Cannot open RPM {rpm}: {e}"))
                })?;
                if !path.is_file() || path.extension().and_then(|v| v.to_str()) != Some("rpm") {
                    return Err(error(
                        ErrorCode::InvalidArgument,
                        "Package install requires a regular .rpm file",
                    ));
                }
                (
                    Command::PackageInstall {
                        local_path: path.to_string_lossy().into_owned(),
                        timeout_ms: timeout * 1000,
                    },
                    None,
                )
            }
            PackageCommand::Uninstall { name } => {
                (Command::PackageUninstall { package: name }, None)
            }
            PackageCommand::Sign { rpm, key, cert } => {
                return emit_local(
                    cli.json,
                    device_id,
                    package::sign(&config, &rpm, key.as_deref(), cert.as_deref())?,
                )
            }
            PackageCommand::Validate { rpm } => {
                return emit_local(cli.json, device_id, package::validate(&rpm)?)
            }
        },
        Commands::Daemon
        | Commands::Shutdown
        | Commands::Install
        | Commands::Uninstall
        | Commands::SetupStatus
        | Commands::Emulator { .. }
        | Commands::Device { .. }
        | Commands::Select { .. } => unreachable!(),
    };
    let raw_shell = matches!(&command, Command::Shell { .. });
    let is_screenshot = matches!(&command, Command::Screenshot { .. });
    let output = daemon::typed_request(device_id, command, cli.command_timeout * 1000).await?;
    if raw_shell && !cli.json {
        if let CommandOutput::Text(text) = output {
            std::io::stdout()
                .write_all(text.as_bytes())
                .map_err(internal)?;
            return Ok(());
        }
    }
    if let CommandOutput::Binary(bytes) = output {
        if let Some(path) = binary_output {
            if is_screenshot {
                audb_core::screenshot::save(&bytes, &path).map_err(core_error)?;
            } else {
                std::fs::write(&path, &bytes).map_err(internal)?;
            }
            let mut metadata = json!({"output":absolute(path),"bytes":bytes.len()});
            if is_screenshot {
                if let Some((width, height)) = audb_core::screenshot::png_dimensions(&bytes) {
                    metadata["format"] = json!("png");
                    metadata["width"] = json!(width);
                    metadata["height"] = json!(height);
                }
            }
            emit(cli.json, device_id, metadata, None);
        } else {
            std::io::stdout().write_all(&bytes).map_err(internal)?;
        }
    } else {
        emit(cli.json, device_id, output_to_value(output), None);
    }
    Ok(())
}

fn map_app(command: AppCommand) -> Command {
    match command {
        AppCommand::Launch { package } => Command::AppLaunch { package },
        AppCommand::Stop { package } => Command::AppStop { package },
        AppCommand::ListRunning => Command::AppListRunning,
        AppCommand::Pid { package } => Command::AppPid { package },
        AppCommand::WaitRunning(v) => Command::AppWait {
            package: v.package,
            running: true,
            timeout_ms: (v.timeout * 1000.0) as u64,
            interval_ms: (v.interval * 1000.0) as u64,
        },
        AppCommand::WaitStopped(v) => Command::AppWait {
            package: v.package,
            running: false,
            timeout_ms: (v.timeout * 1000.0) as u64,
            interval_ms: (v.interval * 1000.0) as u64,
        },
        AppCommand::ClearData {
            package, confirm, ..
        } => Command::AppClearData { package, confirm },
    }
}
fn map_display(command: DisplayCommand) -> Command {
    let (action, timeout) = match command {
        DisplayCommand::Status => return Command::DisplayStatus,
        DisplayCommand::On(v) => ("on", v.timeout),
        DisplayCommand::Off(v) => ("off", v.timeout),
        DisplayCommand::Dim(v) => ("dim", v.timeout),
        DisplayCommand::Lock(v) => ("lock", v.timeout),
        DisplayCommand::Wake(v) => ("wake", v.timeout),
    };
    Command::DisplaySet {
        action: action.into(),
        timeout_ms: (timeout * 1000.0) as u64,
    }
}
fn map_perf(command: PerfCommand, socket: Option<String>) -> Command {
    match command {
        PerfCommand::Snapshot {
            package,
            sample_interval,
        } => Command::PerfSnapshot {
            package,
            sample_interval_ms: (sample_interval * 1000.0) as u64,
        },
        PerfCommand::Monitor {
            package,
            duration,
            interval,
        } => Command::PerfMonitor {
            package,
            duration_ms: (duration * 1000.0) as u64,
            interval_ms: (interval * 1000.0) as u64,
        },
        PerfCommand::VisualFps {
            duration,
            interval,
            freeze_threshold,
        } => Command::VisualFps {
            duration_ms: (duration * 1000.0) as u64,
            interval_ms: (interval * 1000.0) as u64,
            freeze_threshold_ms: (freeze_threshold * 1000.0) as u64,
            socket,
        },
    }
}
fn map_crash(command: CrashCommand) -> Command {
    match command {
        CrashCommand::List {
            package,
            since,
            lines,
        } => Command::CrashList {
            package,
            since,
            lines,
        },
        CrashCommand::Watch {
            package,
            timeout,
            interval,
        } => Command::CrashWatch {
            package,
            timeout_ms: (timeout * 1000.0) as u64,
            interval_ms: (interval * 1000.0) as u64,
        },
        CrashCommand::Clear { package } => Command::CrashClear { package },
    }
}
fn map_sandbox(command: SandboxCommand) -> (Command, Option<PathBuf>) {
    match command {
        SandboxCommand::Paths { package } => (Command::SandboxPaths { package }, None),
        SandboxCommand::List {
            package,
            kind,
            path,
        } => (
            Command::SandboxList {
                package,
                root: kind,
                path,
            },
            None,
        ),
        SandboxCommand::Pull {
            package,
            kind,
            path,
            output,
        } => (
            Command::SandboxPull {
                package,
                root: kind,
                path,
            },
            Some(output),
        ),
        SandboxCommand::Sqlite {
            package,
            kind,
            path,
            query,
        } => (
            Command::SandboxSqlite {
                package,
                root: kind,
                path,
                query,
            },
            None,
        ),
    }
}
fn map_network(command: NetworkCommand) -> Result<Command, AudbError> {
    Ok(match command {
        NetworkCommand::Status => Command::NetworkStatus,
        NetworkCommand::Interfaces => Command::NetworkInterfaces,
        NetworkCommand::Traffic => Command::NetworkTraffic,
        NetworkCommand::Proxy { command } => match command {
            ProxyCommand::Get => Command::NetworkProxyGet,
            ProxyCommand::Set { host, port } => Command::NetworkProxySet { host, port },
            ProxyCommand::Clear => Command::NetworkProxyClear,
        },
        NetworkCommand::Offline { state } => Command::NetworkOffline {
            enabled: parse_on_off(&state)?,
        },
    })
}
fn map_location(command: LocationCommand) -> Result<Command, AudbError> {
    Ok(match command {
        LocationCommand::Set {
            latitude,
            longitude,
            altitude,
        } => Command::LocationSet {
            latitude,
            longitude,
            altitude,
        },
        LocationCommand::Track {
            action,
            value,
            looped,
            speed,
            default_interval,
        } => {
            if action == "load" {
                let path = value.ok_or_else(|| {
                    error(
                        ErrorCode::InvalidArgument,
                        "location track load requires a JSON file",
                    )
                })?;
                let document: Value =
                    serde_json::from_slice(&std::fs::read(path).map_err(internal)?)
                        .map_err(internal)?;
                let positions: Vec<TrackPosition> =
                    serde_json::from_value(document.get("positions").cloned().unwrap_or(document))
                        .map_err(internal)?;
                Command::LocationTrackLoad {
                    positions,
                    looped: looped.map(|v| parse_on_off(&v)).transpose()?,
                    speed,
                    default_interval: default_interval.map(|v| parse_on_off(&v)).transpose()?,
                }
            } else {
                Command::LocationTrackAction {
                    action,
                    index: value
                        .map(|v| v.parse::<i32>())
                        .transpose()
                        .map_err(internal)?,
                    looped: looped.map(|v| parse_on_off(&v)).transpose()?,
                    speed,
                    default_interval: default_interval.map(|v| parse_on_off(&v)).transpose()?,
                }
            }
        }
    })
}
fn map_sensor(command: SensorCommand) -> Command {
    match command {
        SensorCommand::List => Command::SensorList,
        SensorCommand::Enable { sensor } => Command::SensorEnable {
            sensor,
            enabled: true,
        },
        SensorCommand::Disable { sensor } => Command::SensorEnable {
            sensor,
            enabled: false,
        },
        SensorCommand::SetVector { sensor, x, y, z } => Command::SensorVector { sensor, x, y, z },
        SensorCommand::SetScalar { sensor, value } => Command::SensorScalar { sensor, value },
    }
}

fn device_command(
    json_mode: bool,
    command: DeviceCommand,
    target: &mut Option<String>,
) -> Result<(), AudbError> {
    let registry = match command {
        DeviceCommand::List => {
            let r = DeviceRegistry::load().map_err(core_error)?;
            let items: Vec<_> = r
                .devices
                .iter()
                .map(|d| device_value(d, r.resolve(None).ok().map(|d| d.id.as_str())))
                .collect();
            return emit_local(json_mode, None, json!(items));
        }
        DeviceCommand::Current => {
            let r = DeviceRegistry::load().map_err(core_error)?;
            let d = r.resolve(None).map_err(core_error)?;
            *target = Some(d.id.clone());
            return emit_local(json_mode, Some(&d.id), device_value(d, Some(&d.id)));
        }
        DeviceCommand::Add(args) => {
            let d = args.into_config()?;
            let id = d.id.clone();
            let r = DeviceRegistry::transaction(|r| r.add(d)).map_err(core_error)?;
            *target = Some(id.clone());
            return emit_local(
                json_mode,
                Some(&id),
                device_value(
                    r.get(&id).map_err(core_error)?,
                    r.resolve(None).ok().map(|d| d.id.as_str()),
                ),
            );
        }
        DeviceCommand::Update(args) => {
            let id = args.id.clone();
            let check_key = args.key.is_some();
            let r = DeviceRegistry::transaction(|r| {
                let mut d = r.get(&id)?.clone();
                if let Some(v) = args.name {
                    d.name = v;
                }
                if let Some(v) = args.host {
                    d.host = v;
                }
                if let Some(v) = args.port {
                    d.ssh_port = v;
                }
                if let Some(v) = args.user {
                    d.ssh_user = v;
                }
                if args.ssh_config {
                    d.ssh_key = None;
                }
                if let Some(v) = args.key {
                    d.ssh_key = Some(absolute(v));
                }
                if let Some(v) = args.root_user {
                    d.root_user = Some(v);
                }
                if let Some(v) = args.qmp {
                    d.emulator
                        .as_mut()
                        .ok_or_else(|| audb_core::CoreError::invalid("--qmp requires emulator"))?
                        .qmp_socket = absolute(v);
                }
                if let Some(v) = args.sdk_root {
                    d.emulator
                        .as_mut()
                        .ok_or_else(|| {
                            audb_core::CoreError::invalid("--sdk-root requires emulator")
                        })?
                        .sdk_root = absolute(v);
                }
                if let Some(v) = args.emulator_name {
                    d.emulator
                        .as_mut()
                        .ok_or_else(|| {
                            audb_core::CoreError::invalid("--emulator-name requires emulator")
                        })?
                        .emulator_name = v;
                }
                d.validate(check_key)?;
                *r.devices.iter_mut().find(|d| d.id == id).unwrap() = d;
                Ok(())
            })
            .map_err(core_error)?;
            *target = Some(id.clone());
            return emit_local(
                json_mode,
                Some(&id),
                device_value(
                    r.get(&id).map_err(core_error)?,
                    r.resolve(None).ok().map(|d| d.id.as_str()),
                ),
            );
        }
        DeviceCommand::Remove { id } => {
            DeviceRegistry::transaction(|r| r.remove(&id)).map_err(core_error)?;
            *target = Some(id.clone());
            json!({"id":id,"removed":true})
        }
    };
    emit_local(json_mode, target.as_deref(), registry)
}
fn device_value(d: &DeviceConfig, default: Option<&str>) -> Value {
    let mut v = serde_json::to_value(d).expect("serializable device config");
    v["current"] = json!(default == Some(d.id.as_str()));
    v["state"] = json!("unknown"); // Offline registry operation; status probes the selected target.
    v
}
fn parse_on_off(v: &str) -> Result<bool, AudbError> {
    match v {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(error(ErrorCode::InvalidArgument, "expected on or off")),
    }
}
fn output_to_value(output: CommandOutput) -> Value {
    match output {
        CommandOutput::Json(v) => v,
        CommandOutput::Text(v) => json!({"output":v}),
        CommandOutput::Empty => Value::Null,
        CommandOutput::Binary(v) => json!({"bytes":v.len()}),
    }
}
fn emit_local(json_mode: bool, device_id: Option<&str>, value: Value) -> Result<(), AudbError> {
    emit(json_mode, device_id, value.clone(), Some(pretty(&value)));
    Ok(())
}
fn emit(json_mode: bool, device_id: Option<&str>, value: Value, text: Option<String>) {
    if json_mode {
        println!(
            "{}",
            serde_json::to_string(
                &json!({"ok":true,"schemaVersion":1,"deviceId":device_id,"data":value})
            )
            .unwrap()
        )
    } else if let Some(text) = text {
        println!("{text}")
    } else if let Some(text) = value.get("output").and_then(Value::as_str) {
        println!("{text}")
    } else {
        println!("{}", pretty(&value))
    }
}
fn emit_error(json_mode: bool, device_id: Option<&str>, error: &AudbError) {
    if json_mode {
        let mut document = json!({"ok":false,"schemaVersion":1,"deviceId":device_id,"error":{"code":error.code,"message":error.message}});
        if let Some(data) = &error.data {
            document["data"] = data.clone();
        }
        println!("{}", serde_json::to_string(&document).unwrap())
    } else {
        eprintln!("Error: {}", error.message)
    }
}
fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}
fn absolute(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}
fn error(code: ErrorCode, message: impl Into<String>) -> AudbError {
    AudbError {
        code,
        message: message.into(),
        data: None,
    }
}
fn internal(e: impl std::fmt::Display) -> AudbError {
    error(ErrorCode::InternalError, e.to_string())
}
fn core_error(e: audb_core::CoreError) -> AudbError {
    e.into()
}
fn exit_code(code: ErrorCode) -> i32 {
    match code {
        ErrorCode::QmpError => 3,
        ErrorCode::NotFound => 4,
        ErrorCode::SshError => 5,
        ErrorCode::AppNotRunning => 7,
        ErrorCode::AppWaitTimeout => 8,
        ErrorCode::DisplayStateTimeout => 9,
        ErrorCode::CapabilityUnavailable => 10,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_argv_contract_without_shell() {
        let cli =
            Cli::try_parse_from(["audb", "--json", "swipe", "fast-left", "--duration", "300"])
                .unwrap();
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Commands::Swipe {
                duration: Some(300),
                ..
            }
        ));
    }

    #[test]
    fn clear_data_requires_an_explicit_mode() {
        assert!(Cli::try_parse_from(["audb", "app", "clear-data", "ru.example.App"]).is_err());
        assert!(
            Cli::try_parse_from(["audb", "app", "clear-data", "ru.example.App", "--dry-run"])
                .is_ok()
        );
    }

    #[test]
    fn shell_preserves_hyphenated_arguments() {
        let cli = Cli::try_parse_from(["audb", "shell", "journalctl", "--no-pager"]).unwrap();
        assert!(
            matches!(cli.command, Commands::Shell(ShellArgs { cmdline, .. }) if cmdline == ["journalctl", "--no-pager"])
        );
    }

    #[test]
    fn parses_negative_location_and_sensor_values() {
        assert!(Cli::try_parse_from(["audb", "location", "set", "-33.8", "-151.2", "-5"]).is_ok());
        assert!(Cli::try_parse_from([
            "audb",
            "sensor",
            "set-vector",
            "accelerometer",
            "100",
            "-200",
            "-980",
        ])
        .is_ok());
    }
}
