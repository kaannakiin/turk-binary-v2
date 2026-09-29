//! Release-only HTTP measurements over the checked-in Raydium CPMM replay corpus.
//!
//! This is intentionally ignored. It reports observations instead of asserting
//! latency thresholds, because host scheduling and the replay corpus make a
//! timing threshold unsuitable for CI.

use std::alloc::System;
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde::Serialize;
use serde_json::{Value, json};
use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
use tokio::task::JoinSet;
use tower::ServiceExt;

use crate::api::{self, Api};
use crate::{BlockhashSlot, QuoteSettings, QuoteSlot, SearchPool, SwapSettings};

use super::api::universe;

// src: oracle/snapshots/universe.json.gz, slot 451259947 (736 pools, 8 DEXes).
const CORPUS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../oracle/snapshots/universe.json.gz"
);
const CARGO_LOCK: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock");
const WSOL: &str = "So11111111111111111111111111111111111111112";
const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const USER: &str = "J5CBzXpcYn6WR2JBah8zU4Yxct985CAFGwXRcFaX2pbS";
const MAX_CLOCK_STALL: Duration = Duration::from_secs(10);
const DEFAULT_RSS_SAMPLE_INTERVAL_MS: u64 = 10;
const MAX_RSS_REPLAY_SAMPLES: usize = 64;
const MAX_FIXTURE_SETTLE_MS: u64 = 2_000;

#[global_allocator]
static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

#[derive(Debug, Clone, Copy)]
enum Endpoint {
    Quote,
    SwapInstructions,
    Swap,
}

impl Endpoint {
    const ALL: [Self; 3] = [Self::Quote, Self::SwapInstructions, Self::Swap];

    const fn path(self) -> &'static str {
        match self {
            Self::Quote => "/quote",
            Self::SwapInstructions => "/swap-instructions",
            Self::Swap => "/swap",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum RouteMode {
    SingleRouteOnly,
    SplitAllowed,
}

impl RouteMode {
    const ALL: [Self; 2] = [Self::SingleRouteOnly, Self::SplitAllowed];

    const fn label(self) -> &'static str {
        match self {
            Self::SingleRouteOnly => "single-route",
            Self::SplitAllowed => "split-allowed",
        }
    }

    const fn single_route_only(self) -> bool {
        matches!(self, Self::SingleRouteOnly)
    }
}

struct Fixture {
    router: Router,
    corpus_slot: u64,
    corpus_pools: usize,
    corpus_dexes: usize,
    search_max_queued: usize,
    search_timeout_ms: u64,
}

impl Fixture {
    fn new(workers: usize) -> Self {
        let universe = universe::load_from(CORPUS);
        let corpus_slot = universe.slot;
        let corpus_pools = universe.topology.pools().len();
        let corpus_dexes = universe
            .topology
            .pools()
            .iter()
            .map(|pool| pool.dex)
            .collect::<BTreeSet<_>>()
            .len();
        let search_max_queued = configured_max_queued(workers);
        let search_timeout_ms = configured_timeout_ms();
        let quotes = QuoteSlot::default();
        quotes.attach(universe.reader);
        let pool = Arc::new(
            SearchPool::start(workers, search_max_queued).expect("performance search pool starts"),
        );
        let settings = QuoteSettings {
            max_queued: search_max_queued,
            timeout_ms: search_timeout_ms,
            ..QuoteSettings::default()
        };
        let blockhashes = BlockhashSlot::default();
        blockhashes.set(domain::chain::LatestBlockhash {
            hash: [5; 32],
            last_valid_block_height: 1,
        });
        let router = api::router(Api {
            pool,
            quotes,
            settings,
            swap: SwapSettings::default(),
            blockhashes,
            max_clock_stall: MAX_CLOCK_STALL,
            read_timeout: Duration::from_secs(5),
        });
        Self {
            router,
            corpus_slot,
            corpus_pools,
            corpus_dexes,
            search_max_queued,
            search_timeout_ms,
        }
    }
}

#[derive(Debug, Serialize)]
struct Measurement {
    endpoint: &'static str,
    route_mode: &'static str,
    concurrency: usize,
    samples: usize,
    warmup: usize,
    elapsed_ms: u128,
    throughput_per_second: f64,
    p50_us: u128,
    p95_us: u128,
    p99_us: u128,
    successful_samples: usize,
    successful_p50_us: Option<u128>,
    successful_p95_us: Option<u128>,
    successful_p99_us: Option<u128>,
    status_counts: BTreeMap<u16, usize>,
    rss_replay_status_counts: BTreeMap<u16, usize>,
    rss_before_kib: Option<u64>,
    rss_after_kib: Option<u64>,
    rss_peak_kib: Option<u64>,
    allocation_count: usize,
    allocation_bytes: usize,
    deallocation_count: usize,
    deallocation_bytes: usize,
    reallocation_count: usize,
    reallocated_bytes: isize,
}

#[derive(Debug)]
struct Sample {
    status: StatusCode,
    elapsed: Duration,
}

#[tokio::test]
#[ignore = "manual release measurement; reports p50/p95/p99 without CI timing thresholds"]
async fn http_endpoints_report_release_latency_matrix() {
    let workers = configured_workers();
    let samples = configured_count("HTTP_PERF_SAMPLES", 64);
    let warmup = configured_count("HTTP_PERF_WARMUP", 8);
    let metadata_fixture = Fixture::new(workers);
    let git_commit = git_commit().unwrap_or_else(|| "unknown".to_owned());
    let corpus_hash =
        command_output("git", &["hash-object", CORPUS]).unwrap_or_else(|| "unknown".to_owned());
    let cargo_lock_hash =
        command_output("git", &["hash-object", CARGO_LOCK]).unwrap_or_else(|| "unknown".to_owned());
    let rustc = command_output("rustc", &["-Vv"]).unwrap_or_else(|| "unknown".to_owned());
    let machine = command_output("uname", &["-a"]).unwrap_or_else(|| "unknown".to_owned());

    let metadata = json!({
        "corpus": CORPUS,
        "corpus_hash": corpus_hash,
        "cargo_lock_hash": cargo_lock_hash,
        "corpus_slot": metadata_fixture.corpus_slot,
        "corpus_pools": metadata_fixture.corpus_pools,
        "corpus_dexes": metadata_fixture.corpus_dexes,
        "workers": workers,
        "search_max_queued": metadata_fixture.search_max_queued,
        "search_timeout_ms": metadata_fixture.search_timeout_ms,
        "samples": samples,
        "warmup": warmup,
        "rss_replay_samples": samples.min(MAX_RSS_REPLAY_SAMPLES),
        "fixture_isolation": "one fresh search pool per matrix cell",
        "git_commit": git_commit,
        "rustc": rustc,
        "machine": machine,
        "rss_sample_interval_ms": rss_sample_interval_ms(),
        "allocation_instrumentation": "stats_alloc::INSTRUMENTED_SYSTEM",
    });
    println!("http_perf_meta={metadata}");
    drop(metadata_fixture);

    for route_mode in RouteMode::ALL {
        for endpoint in Endpoint::ALL {
            for concurrency in [1, workers, workers.saturating_mul(2)] {
                let body = request_body(endpoint, route_mode);
                let fixture = Fixture::new(workers);
                let measurement = measure(
                    &fixture.router,
                    endpoint,
                    route_mode,
                    &body,
                    concurrency.max(1),
                    samples,
                    warmup,
                )
                .await;
                let settle_ms = fixture.search_timeout_ms.min(MAX_FIXTURE_SETTLE_MS);
                drop(fixture);
                if settle_ms > 0 {
                    std::thread::sleep(Duration::from_millis(settle_ms));
                }
                println!(
                    "http_perf={}",
                    serde_json::to_string(&measurement).expect("measurement serializes")
                );
            }
        }
    }
}

async fn measure(
    router: &Router,
    endpoint: Endpoint,
    route_mode: RouteMode,
    body: &Value,
    concurrency: usize,
    samples: usize,
    warmup: usize,
) -> Measurement {
    let warmup_samples = run_batch(router, endpoint, body, concurrency, warmup).await;

    let rss_before_kib = current_rss_kib();
    let started = Instant::now();
    let (measured, allocation_stats) = {
        let region = Region::new(GLOBAL);
        let measured = run_batch(router, endpoint, body, concurrency, samples).await;
        let allocation_stats = region.change();
        (measured, allocation_stats)
    };
    let elapsed = started.elapsed();
    let rss_after_kib = current_rss_kib();

    let mut latencies = measured
        .iter()
        .map(|sample| sample.elapsed.as_nanos())
        .collect::<Vec<_>>();
    latencies.sort_unstable();
    let mut successful_latencies = measured
        .iter()
        .filter(|sample| sample.status == StatusCode::OK)
        .map(|sample| sample.elapsed.as_nanos())
        .collect::<Vec<_>>();
    successful_latencies.sort_unstable();
    let mut status_counts = BTreeMap::new();
    for sample in &measured {
        *status_counts.entry(sample.status.as_u16()).or_insert(0) += 1;
    }
    let sample_count = measured.len();
    let elapsed_ms = elapsed.as_millis();
    let throughput_per_second =
        f64::from(u32::try_from(measured.len()).expect("measurement count fits u32"))
            / elapsed.as_secs_f64().max(f64::EPSILON);
    drop(measured);
    let (rss_peak_kib, rss_replay_status_counts) = observed_peak_rss(
        router,
        endpoint,
        body,
        concurrency,
        samples.min(MAX_RSS_REPLAY_SAMPLES),
    )
    .await;

    Measurement {
        endpoint: endpoint.path(),
        route_mode: route_mode.label(),
        concurrency,
        samples: sample_count,
        warmup: warmup_samples.len(),
        elapsed_ms,
        throughput_per_second,
        p50_us: percentile_us(&latencies, 50),
        p95_us: percentile_us(&latencies, 95),
        p99_us: percentile_us(&latencies, 99),
        successful_samples: successful_latencies.len(),
        successful_p50_us: percentile_us_if_present(&successful_latencies, 50),
        successful_p95_us: percentile_us_if_present(&successful_latencies, 95),
        successful_p99_us: percentile_us_if_present(&successful_latencies, 99),
        status_counts,
        rss_replay_status_counts,
        rss_before_kib,
        rss_after_kib,
        rss_peak_kib,
        allocation_count: allocation_stats.allocations,
        allocation_bytes: allocation_stats.bytes_allocated,
        deallocation_count: allocation_stats.deallocations,
        deallocation_bytes: allocation_stats.bytes_deallocated,
        reallocation_count: allocation_stats.reallocations,
        reallocated_bytes: allocation_stats.bytes_reallocated,
    }
}

async fn observed_peak_rss(
    router: &Router,
    endpoint: Endpoint,
    body: &Value,
    concurrency: usize,
    request_count: usize,
) -> (Option<u64>, BTreeMap<u16, usize>) {
    let rss_sampler = RssSampler::start();
    let replay = run_batch(router, endpoint, body, concurrency, request_count).await;
    let peak = rss_sampler.finish();
    let mut status_counts = BTreeMap::new();
    for sample in replay {
        *status_counts.entry(sample.status.as_u16()).or_insert(0) += 1;
    }
    (peak, status_counts)
}

struct RssSampler {
    stop: Arc<AtomicBool>,
    peak_kib: Arc<AtomicU64>,
    handle: Option<JoinHandle<()>>,
}

impl RssSampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak_kib = Arc::new(AtomicU64::new(current_rss_kib().unwrap_or(0)));
        let thread_stop = Arc::clone(&stop);
        let thread_peak_kib = Arc::clone(&peak_kib);
        let handle = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                sample_rss(&thread_peak_kib);
                std::thread::sleep(rss_sample_interval());
            }
            sample_rss(&thread_peak_kib);
        });
        Self {
            stop,
            peak_kib,
            handle: Some(handle),
        }
    }

    fn finish(mut self) -> Option<u64> {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take()?.join().ok()?;
        let peak_kib = self.peak_kib.load(Ordering::Relaxed);
        (peak_kib > 0).then_some(peak_kib)
    }
}

fn sample_rss(peak_kib: &AtomicU64) {
    if let Some(rss_kib) = current_rss_kib() {
        peak_kib.fetch_max(rss_kib, Ordering::Relaxed);
    }
}

async fn run_batch(
    router: &Router,
    endpoint: Endpoint,
    body: &Value,
    concurrency: usize,
    count: usize,
) -> Vec<Sample> {
    let mut samples = Vec::with_capacity(count);
    let mut remaining = count;
    while remaining > 0 {
        let batch = remaining.min(concurrency);
        let mut tasks = JoinSet::new();
        for _ in 0..batch {
            let router = router.clone();
            let body = body.clone();
            tasks.spawn(async move { request(router, endpoint.path(), body).await });
        }
        while let Some(result) = tasks.join_next().await {
            samples.push(result.expect("HTTP measurement task completes"));
        }
        remaining -= batch;
    }
    samples
}

async fn request(router: Router, path: &'static str, body: Value) -> Sample {
    let request = Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("valid performance request");
    let started = Instant::now();
    let response = router.oneshot(request).await.expect("router is infallible");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body reads");
    let _: Value = serde_json::from_slice(&bytes).expect("response is JSON");
    Sample {
        status,
        elapsed: started.elapsed(),
    }
}

fn request_body(endpoint: Endpoint, route_mode: RouteMode) -> Value {
    let quote = json!({
        "fromTokenAddress": WSOL,
        "toTokenAddress": USDC,
        "amount": "1000000",
        "slippagePercent": "0.5",
        "singleRouteOnly": route_mode.single_route_only(),
    });
    match endpoint {
        Endpoint::Quote => quote,
        Endpoint::SwapInstructions | Endpoint::Swap => json!({
            "userWalletAddress": USER,
            "wrapAndUnwrapSol": false,
            "quoteRequest": quote,
        }),
    }
}

fn percentile_us(sorted_nanos: &[u128], percentile: usize) -> u128 {
    if sorted_nanos.is_empty() {
        return 0;
    }
    let rank = (sorted_nanos.len() * percentile)
        .div_ceil(100)
        .saturating_sub(1);
    sorted_nanos[rank.min(sorted_nanos.len() - 1)] / 1_000
}

fn percentile_us_if_present(sorted_nanos: &[u128], percentile: usize) -> Option<u128> {
    (!sorted_nanos.is_empty()).then(|| percentile_us(sorted_nanos, percentile))
}

fn configured_workers() -> usize {
    std::env::var("HTTP_PERF_WORKERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&value| value > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map_or(1, std::num::NonZero::get)
                .max(1)
        })
}

fn configured_max_queued(workers: usize) -> usize {
    std::env::var("HTTP_PERF_MAX_QUEUED")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&value| value > 0)
        .unwrap_or_else(|| workers.saturating_mul(2).max(1))
}

fn configured_timeout_ms() -> u64 {
    std::env::var("HTTP_PERF_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|&value| value > 0)
        .unwrap_or(2_000)
}

fn configured_count(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&value| value > 0)
        .unwrap_or(default.max(1))
}

fn rss_sample_interval_ms() -> u64 {
    std::env::var("HTTP_PERF_RSS_INTERVAL_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|&value| value > 0)
        .unwrap_or(DEFAULT_RSS_SAMPLE_INTERVAL_MS)
}

fn rss_sample_interval() -> Duration {
    Duration::from_millis(rss_sample_interval_ms())
}

fn current_rss_kib() -> Option<u64> {
    let pid = std::process::id().to_string();
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

fn git_commit() -> Option<String> {
    command_output("git", &["rev-parse", "HEAD"])
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let output = String::from_utf8(output.stdout).ok()?;
    let output = output.trim();
    (!output.is_empty()).then(|| output.to_owned())
}
