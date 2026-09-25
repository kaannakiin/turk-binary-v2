use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use grpc::{GeyserHub, ProbeKind, SlotSource};
use market::{Engine, MarketError, Universe};
use rpc::RpcGateway;

use crate::{config, output};

pub async fn watch(path: &Path) -> anyhow::Result<()> {
    let config = config::load(path)?;
    let secrets = config::Secrets::from_env()?;
    let rpc = Arc::new(RpcGateway::new(secrets.rpc_url, &config.rpc));

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

    let slot_source = grpc::resolve_slot_source(
        secrets.grpc_url.clone(),
        secrets.grpc_x_token.clone(),
        &config.grpc,
    )
    .await?;
    tracing::info!(?slot_source, "slot statuses");
    let (hub, mut events, hub_task) = GeyserHub::spawn(
        secrets.grpc_url,
        secrets.grpc_x_token,
        &config.grpc,
        slot_source,
    )?;
    let engine = Engine::new(
        &universe,
        rpc,
        hub,
        config.sync.clone(),
        slot_source == SlotSource::BlocksMeta,
    );
    let stats = engine.stats();
    let reader = engine.reader();
    let run = engine.run(&mut events);
    tokio::pin!(run);
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    let period = Duration::from_secs(config.stats_interval_secs.max(1));
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    loop {
        tokio::select! {
            result = &mut run => {
                return match result {
                    Err(MarketError::StreamClosed) => {
                        hub_task.await.context("grpc hub panicked")??;
                        anyhow::bail!("grpc hub stopped")
                    }
                    other => other.context("market sync failed"),
                };
            }
            _ = ticker.tick() => {
                output::log_stats(&stats.snapshot());
                output::log_not_ready(&reader.pools());
            }
            _ = &mut shutdown => {
                tracing::info!("shutting down");
                return Ok(());
            }
        }
    }
}

pub async fn probe(path: &Path, kinds: &[ProbeKind]) -> anyhow::Result<()> {
    let config = config::load(path)?;
    let secrets = config::Secrets::from_env()?;
    let kinds = if kinds.is_empty() {
        &ProbeKind::ALL[..]
    } else {
        kinds
    };
    let findings = grpc::probe(secrets.grpc_url, secrets.grpc_x_token, &config.grpc, kinds).await?;
    output::print_findings(&findings);
    Ok(())
}
