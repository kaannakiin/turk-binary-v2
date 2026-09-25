mod config;
mod output;
mod run;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "Solana cross-DEX arbitrage bot")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Resolve the pool universe, subscribe to every pool's dependencies and keep them in sync (read-only).
    Watch {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Check what the gRPC provider supports (read-only, sends no transactions).
    Probe {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        /// slots, clock, ping-only, replay, limits, slot-backlog. Empty runs them all.
        #[arg(value_delimiter = ',')]
        kinds: Vec<grpc::ProbeKind>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    match Cli::parse().command {
        Command::Watch { config } => run::watch(&config).await,
        Command::Probe { config, kinds } => run::probe(&config, &kinds).await,
    }
}
