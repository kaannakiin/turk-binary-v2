use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufWriter, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::Serialize;

use domain::{ChainClock, DexKind, Pubkey};
use graph::{GraphStats, Topology};
use grpc::{Conn, Finding, GrpcStatsSnapshot, TraceKind, TraceRow};
use market::{PoolView, Readiness, StatsSnapshot, TimingsSnapshot};
use route::{ProbeReport, RouteStatsSnapshot};

pub fn log_grpc(s: &GrpcStatsSnapshot) {
    tracing::info!(
        accounts = s.accounts,
        statuses = s.statuses,
        slots = s.slots,
        lag_p50 = ?s.lag.p50,
        lag_p99 = ?s.lag.p99,
        lag_max = ?s.lag.max,
        blocked_p99 = ?s.blocked.p99,
        blocked_max = ?s.blocked.max,
        queued_peak = s.queued_peak,
        "grpc"
    );
}

pub fn log_stats(s: &StatsSnapshot) {
    let pools = s
        .pool_updates
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(dex, n)| format!("{dex}={n}"))
        .collect::<Vec<_>>()
        .join(" ");
    tracing::info!(
        slot = s.last_slot,
        confirmed = s.confirmed_slot,
        ready = s.pools_ready,
        not_ready = s.pools_not_ready,
        keys = s.keys,
        backlog = s.repair_backlog,
        seeded = s.accounts_seeded,
        seed_failures = s.seeds_failed,
        gaps = s.gaps,
        gap_keys = s.gap_keys,
        downs = s.downs,
        rejected = s.rejected,
        rolled_back = s.rolled_back,
        dead_dropped = s.dead_dropped,
        fork_gaps = s.fork_gaps,
        overflowed = s.overflowed,
        unordered = s.unordered,
        late = s.late,
        txn_orphans = s.txn_orphans,
        txn_superseded = s.txn_superseded,
        views_published = s.views_published,
        stale = s.stale,
        audited = s.audit_checked,
        drift = s.audit_mismatches,
        dependency_updates = s.dependency_updates,
        pool_updates = %pools,
        "stats"
    );
}

pub fn log_engine(s: &TimingsSnapshot) {
    tracing::info!(
        events = s.event.count,
        queued_p50 = ?s.queued.p50,
        queued_p99 = ?s.queued.p99,
        queued_max = ?s.queued.max,
        event_p99 = ?s.event.p99,
        event_max = ?s.event.max,
        fetched_p99 = ?s.fetched.p99,
        fetched_max = ?s.fetched.max,
        tick_p99 = ?s.tick.p99,
        tick_max = ?s.tick.max,
        closures_max = ?s.closures.max,
        readiness_max = ?s.readiness.max,
        "engine"
    );
}

pub fn log_route(s: &RouteStatsSnapshot) {
    tracing::info!(
        pools = s.pools,
        quotable = s.quotable,
        unsupported = s.unsupported,
        decoded = s.decoded,
        decode_errors = s.decode_errors,
        panics = s.panics,
        decode_p50 = ?s.decode.p50,
        decode_p99 = ?s.decode.p99,
        decode_max = ?s.decode.max,
        "route"
    );
}

pub fn log_graph_built(topology: &Topology, took: Duration) {
    let s = topology.stats();
    tracing::info!(
        mints = s.mints,
        pools = s.pools,
        pairs = s.pairs,
        edges = s.edges,
        unplaced = s.unplaced,
        took = ?took,
        "graph built"
    );
    for (pool, reason) in topology.unplaced() {
        tracing::debug!(%pool, ?reason, "pool left out of the graph");
    }
}

pub fn log_graph(s: &GraphStats) {
    tracing::info!(active = s.active, pools = s.pools, flips = s.flips, "graph");
}

pub fn log_probe(amount_in: u64, report: &ProbeReport) {
    let mut summary = String::new();
    for (dex, tally) in &report.by_dex {
        let _ = write!(summary, " {dex}:quoted={}", tally.quoted);
        for (label, n) in &tally.refused {
            let _ = write!(summary, ",{label}={n}");
        }
    }
    tracing::info!(
        amount_in,
        quotes = report.quotes,
        elapsed_us = report.elapsed.as_micros(),
        slowest_us = report.slowest.as_micros(),
        summary = summary.trim_start(),
        "quote probe"
    );
}

#[derive(Serialize)]
struct Snapshot<'a> {
    clock: SnapshotClock,
    pools: Vec<SnapshotPool<'a>>,
}

#[derive(Serialize)]
struct SnapshotClock {
    slot: u64,
    epoch_start_timestamp: i64,
    epoch: u64,
    leader_schedule_epoch: u64,
    unix_timestamp: i64,
}

#[derive(Serialize)]
struct SnapshotPool<'a> {
    pool: String,
    dex: &'a str,
    cross_stream: bool,
    accounts: Vec<SnapshotAccount>,
}

/// `owner` and `data` are absent for an account confirmed not to exist.
#[derive(Serialize)]
struct SnapshotAccount {
    key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    lamports: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
}

pub fn write_snapshot(path: &Path, clock: &ChainClock, views: &[Arc<PoolView>]) -> io::Result<()> {
    let snapshot = Snapshot {
        clock: SnapshotClock {
            slot: clock.slot.0,
            epoch_start_timestamp: clock.epoch_start_timestamp,
            epoch: clock.epoch,
            leader_schedule_epoch: clock.leader_schedule_epoch,
            unix_timestamp: clock.unix_timestamp,
        },
        pools: views
            .iter()
            .map(|view| SnapshotPool {
                pool: view.pool.to_string(),
                dex: view.dex.as_str(),
                cross_stream: view.cross_stream,
                accounts: view
                    .accounts
                    .iter()
                    .filter_map(|(dep, account)| {
                        let account = account.as_ref()?;
                        let exists = account.exists();
                        Some(SnapshotAccount {
                            key: dep.pubkey.to_string(),
                            owner: exists.then(|| account.owner.to_string()),
                            lamports: if exists { account.lamports } else { 0 },
                            data: exists.then(|| STANDARD.encode(&account.data)),
                        })
                    })
                    .collect(),
            })
            .collect(),
    };
    let mut gz = GzEncoder::new(BufWriter::new(File::create(path)?), Compression::best());
    serde_json::to_writer(&mut gz, &snapshot)?;
    gz.finish()?.flush()
}

pub fn log_not_ready(pools: &[(Pubkey, DexKind, Readiness)]) {
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    for (_, dex, readiness) in pools {
        if let Readiness::NotReady(reason) = readiness {
            *reasons.entry(format!("{dex}:{reason:?}")).or_default() += 1;
        }
    }
    if !reasons.is_empty() {
        let summary = reasons
            .iter()
            .map(|(reason, n)| format!("{reason}={n}"))
            .collect::<Vec<_>>()
            .join(" ");
        tracing::info!(%summary, "not ready");
    }
}

#[expect(
    clippy::print_stdout,
    reason = "the probe report is the command's output"
)]
pub fn print_findings(findings: &[Finding]) {
    for f in findings {
        let mark = if f.ok { "ok  " } else { "FAIL" };
        println!("{mark} {:<26} {}", f.check, f.detail);
    }
}

pub const TRACE_HEADER: &str = "conn\trecv_unix_us\tcreated_unix_us\tkind\tslot\twrite_version\tsignature\tpubkey\tslot_status";

pub fn trace_line(row: &TraceRow) -> String {
    fn or_dash<T: ToString>(value: Option<T>) -> String {
        value.map_or_else(|| "-".to_owned(), |v| v.to_string())
    }
    let signature = row.signature.as_deref().map(hex);
    format!(
        "{:?}\t{}\t{}\t{:?}\t{}\t{}\t{}\t{}\t{}",
        row.conn,
        row.recv_unix_us,
        or_dash(row.created_unix_us),
        row.kind,
        row.slot,
        or_dash(row.write_version),
        or_dash(signature),
        or_dash(row.pubkey),
        or_dash(row.slot_status.map(|s| format!("{s:?}"))),
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

pub fn fixture_line(row: &TraceRow) -> Option<String> {
    if row.conn != Conn::Pool {
        return None;
    }
    let sig = row.signature.as_deref().map_or_else(|| "-".to_owned(), hex);
    match row.kind {
        TraceKind::Account => Some(format!(
            "acct\t{}\t{}\t{sig}\t{}\t{}\t{}\t{}",
            row.slot,
            row.write_version?,
            row.pubkey?,
            row.owner?,
            row.lamports?,
            hex(row.data.as_deref()?),
        )),
        TraceKind::Txn => Some(format!(
            "txn\t{}\t{sig}\t{}",
            row.slot,
            row.token_balances
                .iter()
                .map(|(key, amount)| format!("{key}={amount}"))
                .collect::<Vec<_>>()
                .join(","),
        )),
        TraceKind::TxnStatus => Some(format!("status\t{}\t{sig}", row.slot)),
        TraceKind::Slot => None,
    }
}
