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
    /// Measure how a transaction's account writes and its status arrive on one stream (read-only).
    TxnProbe {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
        #[arg(long, default_value_t = 30)]
        minutes: u64,
        #[arg(long, default_value_t = 20)]
        per_dex: usize,
        #[arg(long, default_value_t = 2_000)]
        orphan_after_ms: u64,
        /// Only pools of this DEX, e.g. `raydium_amm_v4`.
        #[arg(long)]
        only: Option<String>,
        /// Write every received message's metadata as TSV.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Write a replay fixture: the pool stream's account bytes, statuses and full transactions.
        #[arg(long)]
        record: Option<PathBuf>,
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
        Command::TxnProbe {
            config,
            minutes,
            per_dex,
            orphan_after_ms,
            only,
            out,
            record,
        } => {
            let args = run::TxnProbeArgs {
                minutes,
                per_dex,
                orphan_after_ms,
                only: only.as_deref(),
                out: out.as_deref(),
                record: record.as_deref(),
            };
            run::txn_probe(&config, args).await
        }
    }
}
