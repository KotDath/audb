mod daemon;

use anyhow::Result;
use audb_protocol::{Command, CommandOutput};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "audb",
    version,
    about = "Aurora Debug Bridge — emulator automation"
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Internal daemon entrypoint.
    #[command(name = "__daemon", hide = true)]
    Daemon,
    /// Check the local audb daemon.
    #[command(hide = true)]
    Ping,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Error: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Daemon => daemon::run().await,
        Commands::Ping => {
            let output = daemon::request(Command::Ping).await?;
            match output {
                CommandOutput::Text(text) => println!("{text}"),
                other => println!("{}", serde_json::to_string(&other)?),
            }
            Ok(())
        }
    }
}
