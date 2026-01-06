use clap::{Parser, Subcommand};

mod features;
mod tools;

#[derive(Parser)]
#[command(name = "audb")]
#[command(about = "Aurora Debug Bridge - Development and debugging CLI tool for Aurora OS", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage Aurora OS devices
    Device {
        #[command(subcommand)]
        action: DeviceCommands,
    },
    /// Select active device
    Select {
        /// Device identifier (name, IP address, or index)
        identifier: String,
    },
    /// Install RPM package on selected device
    Install {
        /// Path to RPM package
        rpm_path: String,
    },
    /// Tap at coordinates on selected device
    Tap {
        /// X coordinate
        x: u16,
        /// Y coordinate
        y: u16,
    },
    /// Swipe gesture on selected device
    Swipe {
        #[command(subcommand)]
        action: SwipeAction,
    },
    /// Take screenshot of selected device (outputs base64-encoded PNG to stdout)
    Screenshot,
    /// Launch an application on selected device
    Launch {
        /// Application name (e.g., ru.auroraos.MLPackLearning)
        app_name: String,
    },
    /// Stop a running application on selected device
    Stop {
        /// Application name (e.g., ru.auroraos.MLPackLearning)
        app_name: String,
    },
    /// Execute shell command on remote device (adb shell style)
    Shell {
        /// Run as root (devel-su)
        #[arg(short, long)]
        root: bool,
        /// Command to execute (required)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Retrieve device logs via journalctl
    Logs {
        /// Number of log lines (default: 100)
        #[arg(short = 'n', long, default_value = "100")]
        lines: usize,
        /// Log level: V/D/I/W/E/F (Android) or debug/info/warning/err (journalctl)
        #[arg(short = 'p', long, value_name = "LEVEL")]
        priority: Option<LogLevel>,
        /// Filter by systemd unit/service (e.g., sailfish-browser.service)
        #[arg(short = 'u', long, value_name = "UNIT")]
        unit: Option<String>,
        /// Search pattern (grep filter)
        #[arg(short = 'g', long)]
        grep: Option<String>,
        /// Show logs since time (e.g., "1 hour ago", "2024-01-06 10:00:00")
        #[arg(short = 's', long)]
        since: Option<String>,
        /// Clear all logs (requires --force to prevent accidents)
        #[arg(long)]
        clear: bool,
        /// Force clear logs without confirmation
        #[arg(long)]
        force: bool,
        /// Show kernel/dmesg messages only
        #[arg(short = 'k', long)]
        kernel: bool,
    },
}

#[derive(Subcommand)]
enum DeviceCommands {
    /// List all devices
    List {
        /// Show only active (reachable) devices
        #[arg(short, long)]
        active: bool,
    },
    /// Add a new device interactively
    Add,
    /// Remove a device
    Remove {
        /// Device identifier (name, IP address, or index)
        identifier: String,
    },
}

#[derive(Subcommand)]
enum SwipeAction {
    /// Swipe between explicit coordinates
    Coords {
        x1: u16,
        y1: u16,
        x2: u16,
        y2: u16,
    },
    /// Swipe in named direction
    Direction {
        #[clap(value_enum)]
        direction: SwipeDirectionArg,
    },
}

#[derive(clap::ValueEnum, Clone, Debug)]
#[clap(rename_all = "lowercase")]
enum SwipeDirectionArg {
    Left,
    Right,
    Up,
    Down,
}

impl From<SwipeDirectionArg> for features::input::swipe::SwipeDirection {
    fn from(arg: SwipeDirectionArg) -> Self {
        match arg {
            SwipeDirectionArg::Left => Self::Left,
            SwipeDirectionArg::Right => Self::Right,
            SwipeDirectionArg::Up => Self::Up,
            SwipeDirectionArg::Down => Self::Down,
        }
    }
}

#[derive(clap::ValueEnum, Clone, Debug)]
#[clap(rename_all = "lowercase")]
enum LogLevel {
    V,
    D,
    I,
    W,
    E,
    F,
    Debug,
    Info,
    Notice,
    Warning,
    Err,
    Crit,
    Alert,
    Emerg,
}

impl LogLevel {
    fn to_journalctl_priority(&self) -> &str {
        match self {
            Self::V | Self::D | Self::Debug => "debug",
            Self::I | Self::Info => "info",
            Self::Notice => "notice",
            Self::W | Self::Warning => "warning",
            Self::E | Self::Err => "err",
            Self::F | Self::Crit => "crit",
            Self::Alert => "alert",
            Self::Emerg => "emerg",
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Device { action } => match action {
            DeviceCommands::List { active } => {
                features::device::list::execute(active).await
            }
            DeviceCommands::Add => {
                features::device::add::execute().await
            }
            DeviceCommands::Remove { identifier } => {
                features::device::remove::execute(&identifier).await
            }
        },
        Commands::Select { identifier } => {
            features::device::select::execute(&identifier).await
        }
        Commands::Install { rpm_path } => {
            features::install::rpm::execute(&rpm_path).await
        }
        Commands::Tap { x, y } => {
            features::input::tap::execute(x, y).await
        }
        Commands::Swipe { action } => {
            match action {
                SwipeAction::Coords { x1, y1, x2, y2 } => {
                    features::input::swipe::execute(
                        features::input::swipe::SwipeMode::Coords { x1, y1, x2, y2 }
                    ).await
                }
                SwipeAction::Direction { direction } => {
                    features::input::swipe::execute(
                        features::input::swipe::SwipeMode::Direction(direction.into())
                    ).await
                }
            }
        }
        Commands::Screenshot => {
            features::input::screenshot::execute().await
        }
        Commands::Launch { app_name } => {
            features::app::launch::execute(&app_name).await
        }
        Commands::Stop { app_name } => {
            features::app::stop::execute(&app_name).await
        }
        Commands::Shell { root, command } => {
            let cmd = command.join(" ");
            features::shell::execute(root, cmd).await
        }
        Commands::Logs { lines, priority, unit, grep, since, clear, force, kernel } => {
            features::logs::execute(features::logs::LogsArgs {
                lines,
                priority,
                unit,
                grep,
                since,
                clear,
                force,
                kernel,
            }).await
        }
    };

    if let Err(e) = result {
        exit_error!("{}", e);
    }
}
