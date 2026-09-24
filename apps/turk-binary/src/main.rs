mod config;
mod output;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};
use grpc::GeyserHub;
use market::{AccountStore, MarketError, Stats, Universe};
use rpc::RpcGateway;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(version, about = "Solana cross-DEX arbitrage bot")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Resolve the configured pool universe and stream live pool updates (read-only).
    Watch {
        #[arg(long, default_value = "config.toml")]
        config: PathBuf,
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
        Command::Watch { config } => watch(&config).await,
    }
}

async fn watch(path: &Path) -> anyhow::Result<()> {
    let config = config::load(path)?;
    let secrets = config::Secrets::from_env()?;
    let rpc = RpcGateway::new(secrets.rpc_url, &config.rpc);

    let universe = Universe::resolve(&config.universe, &rpc)
        .await
        .context("resolving pool universe")?;
    for (dex, pools) in universe.count_by_dex() {
        tracing::info!(%dex, pools, "universe");
    }
    anyhow::ensure!(
        !universe.pools.is_empty(),
        "no pools matched the configured universe"
    );

    let (hub, mut events, hub_task) =
        GeyserHub::spawn(secrets.grpc_url, secrets.grpc_x_token, &config.grpc)?;
    let store = AccountStore::default();
    let stats = Stats::default();
    let ingest = market::run(&universe, &rpc, &hub, &mut events, &store, &stats);
    tokio::pin!(ingest);
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    let period = Duration::from_secs(config.stats_interval_secs.max(1));
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);

    loop {
        tokio::select! {
            result = &mut ingest => {
                return match result {
                    Err(MarketError::StreamClosed) => {
                        hub_task.await.context("grpc hub panicked")??;
                        anyhow::bail!("grpc hub stopped")
                    }
                    other => other.context("market ingestion failed"),
                };
            }
            _ = ticker.tick() => output::log_stats(&stats.snapshot(), store.len()),
            _ = &mut shutdown => {
                tracing::info!("shutting down");
                return Ok(());
            }
        }
    }
}
