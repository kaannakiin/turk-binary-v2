use std::collections::BTreeMap;
use std::fs::File;
use std::future::Future;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use domain::{DexKind, Pubkey};
use graph::Topology;
use grpc::{GeyserHub, ProbeKind, SlotSource, TraceRow, TxnProbeOptions};
use market::{Market, MarketError, MarketReader, Readiness, Universe};
use route::{Decoding, ProbeReport, QuoteReader};
use rpc::RpcGateway;
use tokio::task::JoinHandle;

use crate::{config, output};

struct Running {
    hub: JoinHandle<Result<(), grpc::GrpcError>>,
    grpc: grpc::GrpcStats,
    market: Market,
    decoding: Decoding,
    quotes: QuoteReader<MarketReader>,
    reader: MarketReader,
    topology: Arc<Topology>,
    rpc: Arc<RpcGateway>,
    config: config::AppConfig,
}

impl Running {
    async fn start(config: config::AppConfig) -> anyhow::Result<Self> {
        let secrets = config::Secrets::from_env()?;
        let rpc = Arc::new(RpcGateway::new(secrets.rpc_url, &config.rpc)?);

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
        let started = Instant::now();
        let topology =
            Arc::new(Topology::from_universe(&universe).context("building the token graph")?);
        output::log_graph_built(&topology, started.elapsed());

        let slot_source = grpc::resolve_slot_source(
            secrets.grpc_url.clone(),
            secrets.grpc_x_token.clone(),
            &config.grpc,
        )
        .await?;
        tracing::info!(?slot_source, "slot statuses");
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let partitions =
            market::pipeline_threads(config.threads.pipeline, config.grpc.streams, cores)?;
        tracing::info!(
            pipeline = partitions,
            streams = config.grpc.streams,
            cores,
            "threads"
        );
        let hub = GeyserHub::spawn(
            secrets.grpc_url,
            secrets.grpc_x_token,
            &config.grpc,
            slot_source,
            partitions,
        )?;
        let mut decoding = Decoding::new(Arc::clone(&topology));
        let market = Market::start(
            &universe,
            &rpc,
            hub.partitions,
            &config.sync,
            slot_source == SlotSource::BlocksMeta,
            |_| Box::new(decoding.decoder()),
        )?;
        let reader = market.reader();
        Ok(Self {
            hub: hub.task,
            grpc: hub.stats,
            market,
            quotes: decoding.reader(reader.clone()),
            decoding,
            reader,
            topology,
            rpc,
            config,
        })
    }

    /// Logs stats every tick until ctrl-c, a failure, or `until` fires.
    async fn run(&mut self, until: impl Future<Output = ()>) -> anyhow::Result<()> {
        let shutdown = tokio::signal::ctrl_c();
        tokio::pin!(shutdown, until);
        let period = Duration::from_secs(self.config.stats_interval_secs.max(1));
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        let quotes = self.quotes.clone();
        let probe_amount = self.config.route.probe_amount;
        let mut probing: Option<JoinHandle<ProbeReport>> = None;
        loop {
            tokio::select! {
                result = self.market.stopped() => {
                    return match result {
                        Err(MarketError::StreamClosed) => {
                            (&mut self.hub).await.context("grpc hub panicked")??;
                            anyhow::bail!("grpc hub stopped")
                        }
                        other => other.context("market sync failed"),
                    };
                }
                _ = ticker.tick() => {
                    output::log_grpc(&self.grpc.snapshot());
                    output::log_stats(&self.market.stats());
                    output::log_engine(&self.market.timings());
                    output::log_route(&self.decoding.stats());
                    output::log_graph(&self.topology.stats());
                    output::log_not_ready(&self.reader.pools());
                    if probe_amount > 0 && probing.as_ref().is_none_or(JoinHandle::is_finished) {
                        if let Some(done) = probing.take() {
                            output::log_probe(probe_amount, &done.await.context("quote probe panicked")?);
                        }
                        let quotes = quotes.clone();
                        probing = Some(tokio::task::spawn_blocking(move || {
                            quotes.probe(probe_amount, u8::MAX)
                        }));
                    }
                }
                () = &mut until => return Ok(()),
                _ = &mut shutdown => {
                    tracing::info!("shutting down");
                    return Ok(());
                }
            }
        }
    }
}

pub async fn watch(path: &Path) -> anyhow::Result<()> {
    Running::start(config::load(path)?)
        .await?
        .run(std::future::pending())
        .await
}

type Servers = (
    JoinHandle<Result<(), server::ServerError>>,
    JoinHandle<Result<(), server::ServerError>>,
);

/// `watch` plus the quote API and the ops endpoints. Both answer from before
/// the engine starts until it has stopped; the engine outlives them, so no
/// request is priced on state that stopped updating.
pub async fn serve(path: &Path) -> anyhow::Result<()> {
    let config = config::load(path)?;
    let settings = config.server.clone();
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let threads = server::search_threads(config.threads.search, cores);
    let health = server::Health::new(settings.ready);
    let quotes = server::QuoteSlot::default();
    let blockhashes = server::BlockhashSlot::default();
    let pool = server::SearchPool::start(threads, settings.quote.max_queued)?;
    let api = server::ApiServer::bind(
        &settings,
        health.clone(),
        pool,
        quotes.clone(),
        blockhashes.clone(),
    )
    .await?;
    let ops = server::OpsServer::bind(&settings, health.clone()).await?;
    tracing::info!(api = %api.local_addr()?, ops = %ops.local_addr()?, search = threads, "listening");
    let servers = (tokio::spawn(api.run()), tokio::spawn(ops.run()));
    let stop = shutdown_signal();
    tokio::pin!(stop);
    let mut running = tokio::select! {
        running = Running::start(config) => running?,
        () = &mut stop => return stop_servers(&health, servers).await,
    };
    quotes.attach(running.quotes.clone());
    let rpc = Arc::clone(&running.rpc);
    let refresh = tokio::spawn(
        blockhashes.refresh(settings.swap.blockhash_refresh(), move || {
            let rpc = Arc::clone(&rpc);
            async move { rpc.get_latest_blockhash().await }
        }),
    );
    health.serving(running.reader.clone());
    let result = running.run(&mut stop).await;
    refresh.abort();
    health.drain();
    if result.is_ok() {
        tokio::time::sleep(settings.drain_delay()).await;
    }
    stop_servers(&health, servers).await?;
    drop(running);
    result
}

async fn stop_servers(health: &server::Health, (api, ops): Servers) -> anyhow::Result<()> {
    health.stop();
    api.await.context("api server panicked")??;
    ops.await.context("ops server panicked")??;
    Ok(())
}

async fn shutdown_signal() {
    let interrupt = async {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    };
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::warn!(%err, "no SIGTERM handler; only ctrl-c stops the server");
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("shutting down");
}

#[derive(clap::Args)]
pub struct SnapshotArgs {
    #[arg(long, default_value = "config.toml")]
    config: PathBuf,
    /// How long the market syncs before the views are taken.
    #[arg(long, default_value_t = 90)]
    settle_secs: u64,
    #[arg(long, default_value_t = 12)]
    per_dex: usize,
    /// Every ready pool instead of `per_dex` of each DEX.
    #[arg(long)]
    all: bool,
    #[arg(long)]
    out: PathBuf,
}

/// Lets the market settle, then writes `per_dex` ready pools of every DEX
/// (or all of them) with the accounts their views hold and the Clock.
pub async fn snapshot(args: &SnapshotArgs) -> anyhow::Result<()> {
    let per_dex = if args.all { usize::MAX } else { args.per_dex };
    let out = args.out.as_path();
    let mut running = Running::start(config::load(&args.config)?).await?;
    running
        .run(tokio::time::sleep(Duration::from_secs(args.settle_secs)))
        .await?;
    let clock = running.reader.clock().context("no Clock sysvar yet")?;
    let mut by_dex: BTreeMap<DexKind, Vec<Pubkey>> = BTreeMap::new();
    for (pool, dex, readiness) in running.reader.pools() {
        if readiness == Readiness::Ready {
            by_dex.entry(dex).or_default().push(pool);
        }
    }
    let views: Vec<_> = by_dex
        .into_values()
        .flat_map(|mut pools| {
            pools.sort_unstable();
            let step = pools.len().div_ceil(per_dex.max(1)).max(1);
            pools.into_iter().step_by(step).collect::<Vec<_>>()
        })
        .filter_map(|pool| running.reader.pool_view(&pool))
        .collect();
    output::write_snapshot(out, &clock, &views).context("writing the snapshot")?;
    tracing::info!(pools = views.len(), slot = clock.slot.0, out = %out.display(), "snapshot");
    Ok(())
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

#[derive(clap::Args)]
pub struct TxnProbeArgs {
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
}

pub async fn txn_probe(args: &TxnProbeArgs) -> anyhow::Result<()> {
    let config = config::load(&args.config)?;
    let secrets = config::Secrets::from_env()?;
    let rpc = RpcGateway::new(secrets.rpc_url, &config.rpc)?;
    let universe = Universe::resolve(&config.universe, &rpc)
        .await
        .context("resolving pool universe")?;
    let targets: Vec<_> = universe
        .probe_targets(args.per_dex)
        .into_iter()
        .filter(|t| args.only.as_deref().is_none_or(|only| t.label == only))
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
    let mut trace_out = create(args.out.as_deref())?;
    let mut record_out = create(args.record.as_deref())?;
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
