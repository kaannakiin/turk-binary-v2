use market::StatsSnapshot;

pub fn log_stats(snapshot: &StatsSnapshot, accounts: usize) {
    let per_dex = snapshot
        .per_dex
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(dex, n)| format!("{dex}={n}"))
        .collect::<Vec<_>>()
        .join(" ");
    tracing::info!(
        slot = snapshot.last_slot.0,
        confirmed = snapshot.confirmed_slot.0,
        rolled_back = snapshot.rolled_back,
        dead_dropped = snapshot.dead_dropped,
        fork_gaps = snapshot.fork_gaps,
        accounts,
        stale = snapshot.stale,
        other = snapshot.other,
        reconnects = snapshot.reconnects,
        updates = %per_dex,
        "stats"
    );
}
