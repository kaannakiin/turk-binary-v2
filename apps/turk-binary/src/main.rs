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
    /// `watch` plus the quote API and `/health`, `/ready` (read-only, builds no transactions).
    Serve {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Check what the gRPC provider supports (read-only, sends no transactions).
    Probe {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        /// slots, clock, ping-only, limits, slot-backlog. Empty runs them all.
        #[arg(value_delimiter = ',')]
        kinds: Vec<grpc::ProbeKind>,
    },
    /// Measure how a transaction's account writes and its status arrive on one stream (read-only).
    TxnProbe(run::TxnProbeArgs),
    /// Write ready pools' account views and the Clock, for the `LiteSVM` oracle (read-only).
    Snapshot(run::SnapshotArgs),
}

/// Startup, the stats ticker and the slot feed; streams and decoding run on
/// the pipeline threads.
const APP_THREADS: usize = 2;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let command = Cli::parse().command;
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(APP_THREADS)
        .thread_name("app")
        .enable_all()
        .build()?
        .block_on(run(command))
}

async fn run(command: Command) -> anyhow::Result<()> {
    match command {
        Command::Watch { config } => run::watch(&config).await,
        Command::Serve { config } => run::serve(&config).await,
        Command::Probe { config, kinds } => run::probe(&config, &kinds).await,
        Command::Snapshot(args) => run::snapshot(&args).await,
        Command::TxnProbe(args) => run::txn_probe(&args).await,
    }
}
