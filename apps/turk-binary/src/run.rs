use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use grpc::{GeyserHub, ProbeKind, SlotSource, TraceRow, TxnProbeOptions};
use market::{Market, MarketError, Universe};
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
    let partitions = config.sync.partitions(config.grpc.streams)?;
    let hub = GeyserHub::spawn(
        secrets.grpc_url,
        secrets.grpc_x_token,
        &config.grpc,
        slot_source,
        partitions,
    )?;
    let mut market = Market::start(
        &universe,
        &rpc,
        hub.partitions,
        &config.sync,
        slot_source == SlotSource::BlocksMeta,
    )?;
    tracing::info!(partitions, "market partitions");
    let reader = market.reader();
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    let period = Duration::from_secs(config.stats_interval_secs.max(1));
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
    loop {
        tokio::select! {
            result = market.stopped() => {
                return match result {
                    Err(MarketError::StreamClosed) => {
                        hub.task.await.context("grpc hub panicked")??;
                        anyhow::bail!("grpc hub stopped")
                    }
                    other => other.context("market sync failed"),
                };
            }
            _ = ticker.tick() => {
                output::log_stats(&market.stats());
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

pub struct TxnProbeArgs<'a> {
    pub minutes: u64,
    pub per_dex: usize,
    pub orphan_after_ms: u64,
    pub only: Option<&'a str>,
    pub out: Option<&'a Path>,
    pub record: Option<&'a Path>,
}

pub async fn txn_probe(path: &Path, args: TxnProbeArgs<'_>) -> anyhow::Result<()> {
    let config = config::load(path)?;
    let secrets = config::Secrets::from_env()?;
    let rpc = RpcGateway::new(secrets.rpc_url, &config.rpc);
    let universe = Universe::resolve(&config.universe, &rpc)
        .await
        .context("resolving pool universe")?;
    let targets: Vec<_> = universe
        .probe_targets(args.per_dex)
        .into_iter()
        .filter(|t| args.only.is_none_or(|only| t.label == only))
        .collect();
    anyhow::ensure!(!targets.is_empty(), "no pools to probe");
    let slot_source = grpc::resolve_slot_source(
        secrets.grpc_url.clone(),
        secrets.grpc_x_token.clone(),
        &config.grpc,
    )
    .await?;
    tracing::info!(
        pools = targets.len(),
        minutes = args.minutes,
        ?slot_source,
        "txn probe"
    );
    let options = TxnProbeOptions {
        duration: Duration::from_secs(args.minutes.saturating_mul(60)),
        orphan_after: Duration::from_millis(args.orphan_after_ms),
        slot_statuses: slot_source == SlotSource::Slots,
        record: args.record.is_some(),
    };
    let create = |path: Option<&Path>| {
        path.map(|p| File::create(p).map(BufWriter::new))
            .transpose()
            .context("creating an output file")
    };
    let mut trace_out = create(args.out)?;
    let mut record_out = create(args.record)?;
    if let Some(w) = trace_out.as_mut() {
        writeln!(w, "{}", output::TRACE_HEADER).context("writing the trace file")?;
    }
    let mut write_error = None;
    let findings = {
        let mut trace = |row: TraceRow| {
            if write_error.is_some() {
                return;
            }
            let written = trace_out
                .as_mut()
                .map_or(Ok(()), |w| writeln!(w, "{}", output::trace_line(&row)))
                .and_then(
                    |()| match (record_out.as_mut(), output::fixture_line(&row)) {
                        (Some(w), Some(line)) => writeln!(w, "{line}"),
                        _ => Ok(()),
                    },
                );
            if let Err(err) = written {
                write_error = Some(err);
            }
        };
        grpc::probe_txn_groups(
            secrets.grpc_url,
            secrets.grpc_x_token,
            &config.grpc,
            &targets,
            options,
            &mut trace,
        )
        .await?
    };
    if let Some(err) = write_error {
        return Err(err).context("writing an output file");
    }
    for w in [trace_out, record_out].into_iter().flatten() {
        w.into_inner()
            .map_err(std::io::IntoInnerError::into_error)
            .context("writing an output file")?;
    }
    output::print_findings(&findings);
    Ok(())
}
