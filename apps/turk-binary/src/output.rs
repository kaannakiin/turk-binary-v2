use std::collections::BTreeMap;
use std::fmt::Write as _;

use domain::{DexKind, Pubkey};
use grpc::{Conn, Finding, TraceKind, TraceRow};
use market::{Readiness, StatsSnapshot};

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
        resumed = s.resumed,
        rejected = s.rejected,
        rolled_back = s.rolled_back,
        dead_dropped = s.dead_dropped,
        fork_gaps = s.fork_gaps,
        overflowed = s.overflowed,
        late = s.late,
        txn_orphans = s.txn_orphans,
        stale = s.stale,
        audited = s.audit_checked,
        drift = s.audit_mismatches,
        dependency_updates = s.dependency_updates,
        pool_updates = %pools,
        "stats"
    );
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
