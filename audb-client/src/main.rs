mod daemon;
mod package;

use audb_core::{config::EMULATOR_ID, emulator, setup, EmulatorConfig};
use audb_protocol::{
    AudbError, Command, CommandOutput, ErrorCode, LogsOptions, SwipeOptions, TrackPosition,
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
    about = "Aurora Debug Bridge — emulator automation"
)]
struct Cli {
    #[arg(long, global = true, default_value = "/tmp/audb/qmp.sock")]
    socket: String,
    #[arg(short = 'd', long, global = true)]
    device: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
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
        string: String,
        #[arg(long, default_value_t = 50)]
        delay: u64,
    },
    Key {
        name: String,
    },
    Screenshot {
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    Status,
    Install,
    Uninstall,
    SetupStatus,
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
    Add {
        #[arg(long)]
        id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        host: String,
        #[arg(long, default_value_t = 22)]
        port: u16,
        #[arg(long)]
        key: String,
        #[arg(long, default_value = "defaultuser")]
        user: String,
        #[arg(long, default_value = "physical")]
        kind: String,
        #[arg(long)]
        qmp: Option<String>,
    },
    Remove {
        id: String,
    },
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
        latitude: f64,
        longitude: f64,
        #[arg(default_value_t = 0.0)]
        altitude: f64,
    },
    Track {
        action: String,
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
        x: i32,
        y: i32,
        z: i32,
    },
    SetScalar {
        sensor: String,
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
    },
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
                &error(ErrorCode::InvalidArgument, parse_error.to_string()),
            );
            std::process::exit(1);
        }
        Err(parse_error) => parse_error.exit(),
    };
    let json_mode = cli.json;
    match run(cli).await {
        Ok(()) => {}
        Err(error) => {
            emit_error(json_mode, &error);
            std::process::exit(exit_code(error.code));
        }
    }
}

async fn run(cli: Cli) -> Result<(), AudbError> {
    if let Some(device) = &cli.device {
        require_emulator(device)?;
    }
    let mut config = EmulatorConfig::load_or_default().map_err(core_error)?;
    config.qmp_socket = cli.socket.clone().into();
    match cli.command {
        Commands::Daemon => return daemon::run().await.map_err(internal),
        Commands::Shutdown => {
            let output = daemon::typed_request(Command::Shutdown).await?;
            emit(cli.json, output_to_value(output), None);
            return Ok(());
        }
        Commands::Install => {
            return emit_local(cli.json, setup::install(&config).map_err(core_error)?)
        }
        Commands::Uninstall => {
            return emit_local(cli.json, setup::uninstall(&config).map_err(core_error)?)
        }
        Commands::SetupStatus => {
            return emit_local(cli.json, setup::status(&config).map_err(core_error)?)
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
            return emit_local(cli.json, value);
        }
        Commands::Device { command } => return device_command(cli.json, command, &config).await,
        Commands::Select { id } => {
            require_emulator(&id)?;
            return emit_local(cli.json, json!({"id":EMULATOR_ID,"selected":true}));
        }
        _ => {}
    }

    let (command, binary_output): (Command, Option<PathBuf>) = match cli.command {
        Commands::Tap { x, y, duration } => (
            Command::Tap {
                x,
                y,
                duration_ms: duration,
                socket: Some(cli.socket),
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
                socket: Some(cli.socket),
            },
            None,
        ),
        Commands::Text { string, delay } => (
            Command::Text {
                text: string,
                delay_ms: delay,
                socket: Some(cli.socket),
            },
            None,
        ),
        Commands::Key { name } => (
            Command::Key {
                name,
                socket: Some(cli.socket),
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
            (
                Command::Screenshot {
                    socket: Some(cli.socket),
                },
                output,
            )
        }
        Commands::Status => (
            Command::QmpStatus {
                socket: Some(cli.socket),
            },
            None,
        ),
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
            PackageCommand::List { filter } => (Command::PackageList { filter }, None),
            PackageCommand::Install { rpm } => (
                Command::PackageInstall {
                    name: rpm.clone(),
                    bytes: std::fs::read(&rpm).map_err(internal)?,
                },
                None,
            ),
            PackageCommand::Uninstall { name } => {
                (Command::PackageUninstall { package: name }, None)
            }
            PackageCommand::Sign { rpm, key, cert } => {
                return emit_local(
                    cli.json,
                    package::sign(&config, &rpm, key.as_deref(), cert.as_deref())?,
                )
            }
            PackageCommand::Validate { rpm } => {
                return emit_local(cli.json, package::validate(&rpm)?)
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
    let output = daemon::typed_request(command).await?;
    if let CommandOutput::Binary(bytes) = output {
        if let Some(path) = binary_output {
            std::fs::write(&path, &bytes).map_err(internal)?;
            emit(
                cli.json,
                json!({"output":absolute(path),"bytes":bytes.len()}),
                None,
            );
        } else {
            std::io::stdout().write_all(&bytes).map_err(internal)?;
        }
    } else {
        emit(cli.json, output_to_value(output), None);
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
fn map_perf(command: PerfCommand, socket: String) -> Command {
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
            socket: Some(socket),
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

async fn device_command(
    json_mode: bool,
    command: DeviceCommand,
    config: &EmulatorConfig,
) -> Result<(), AudbError> {
    match command {
        DeviceCommand::List => {
            let item = json!({"id":EMULATOR_ID,"name":config.name,"kind":"emulator","host":config.host,"sshPort":config.ssh_port,"qmpSocket":config.qmp_socket,"current":true,"state":if emulator::is_running(config).await{"online"}else{"offline"}});
            emit_local(json_mode, Value::Array(vec![item]))
        }
        DeviceCommand::Current => emit_local(
            json_mode,
            json!({"id":EMULATOR_ID,"name":config.name,"kind":"emulator","host":config.host,"sshPort":config.ssh_port,"qmpSocket":config.qmp_socket,"current":true,"state":if emulator::is_running(config).await{"online"}else{"offline"}}),
        ),
        DeviceCommand::Add { id, .. } | DeviceCommand::Remove { id } => Err(error(
            ErrorCode::UnsupportedInEmulatorOnly,
            format!("Device registry mutation is unavailable in emulator-only mode: {id}"),
        )),
    }
}
fn require_emulator(id: &str) -> Result<(), AudbError> {
    if id == EMULATOR_ID {
        Ok(())
    } else {
        Err(error(
            ErrorCode::UnsupportedInEmulatorOnly,
            format!("Only device '{EMULATOR_ID}' is supported"),
        ))
    }
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
fn emit_local(json_mode: bool, value: Value) -> Result<(), AudbError> {
    emit(json_mode, value.clone(), Some(pretty(&value)));
    Ok(())
}
fn emit(json_mode: bool, value: Value, text: Option<String>) {
    if json_mode {
        println!(
            "{}",
            serde_json::to_string(&json!({"ok":true,"deviceId":EMULATOR_ID,"data":value})).unwrap()
        )
    } else if let Some(text) = text {
        println!("{text}")
    } else if let Some(text) = value.get("output").and_then(Value::as_str) {
        println!("{text}")
    } else {
        println!("{}", pretty(&value))
    }
}
fn emit_error(json_mode: bool, error: &AudbError) {
    if json_mode {
        println!("{}",serde_json::to_string(&json!({"ok":false,"deviceId":EMULATOR_ID,"error":{"code":error.code,"message":error.message}})).unwrap())
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
}
