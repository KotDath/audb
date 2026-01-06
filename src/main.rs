use clap::{Parser, Subcommand};

mod features;
mod tools;

#[derive(Parser)]
#[command(name = "audb")]
#[command(about = "Aurora OS Device Manager - Device management CLI tool for Aurora OS", long_about = None)]
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
    /// Execute shell command on remote device (adb shell style)
    Shell {
        /// Run as root (devel-su)
        #[arg(short, long)]
        root: bool,
        /// Command to execute (required)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
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
        Commands::Shell { root, command } => {
            let cmd = command.join(" ");
            features::shell::execute(root, cmd).await
        }
    };

    if let Err(e) = result {
        exit_error!("{}", e);
    }
}
