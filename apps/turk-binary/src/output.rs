use std::collections::BTreeMap;

use domain::{DexKind, Pubkey};
use grpc::Finding;
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
