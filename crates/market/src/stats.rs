use std::sync::atomic::{AtomicU64, Ordering};

use domain::{DexKind, Slot};

use crate::store::{Applied, Resolved};

macro_rules! counters {
    ($($name:ident),* $(,)?) => {
        #[derive(Debug, Default)]
        pub struct Stats {
            pool_updates: [AtomicU64; DexKind::ALL.len()],
            $($name: AtomicU64,)*
        }

        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct StatsSnapshot {
            pub pool_updates: Vec<(DexKind, u64)>,
            $(pub $name: u64,)*
        }

        impl Stats {
            #[must_use]
            pub fn snapshot(&self) -> StatsSnapshot {
                StatsSnapshot {
                    pool_updates: DexKind::ALL
                        .into_iter()
                        .map(|d| (d, self.pool_updates[d as usize].load(Ordering::Relaxed)))
                        .collect(),
                    $($name: self.$name.load(Ordering::Relaxed),)*
                }
            }
        }

        impl StatsSnapshot {
            /// Partitions' counters add up; the slots they report are the
            /// newest any of them saw.
            #[must_use]
            pub fn merge(parts: &[Self]) -> Self {
                let mut merged = Self {
                    pool_updates: DexKind::ALL
                        .into_iter()
                        .map(|d| (d, parts.iter().map(|p| p.pool_updates[d as usize].1).sum()))
                        .collect(),
                    $($name: parts.iter().map(|p| p.$name).sum(),)*
                };
                merged.last_slot = parts.iter().map(|p| p.last_slot).max().unwrap_or(0);
                merged.confirmed_slot = parts.iter().map(|p| p.confirmed_slot).max().unwrap_or(0);
                merged
            }
        }
    };
}

counters!(
    dependency_updates,
    stale,
    overflowed,
    late,
    last_slot,
    confirmed_slot,
    rolled_back,
    dead_dropped,
    fork_gaps,
    downs,
    resumed,
    gaps,
    gap_keys,
    rejected,
    seeds_sent,
    seeds_failed,
    accounts_seeded,
    audit_checked,
    audit_mismatches,
    keys,
    pools_ready,
    pools_not_ready,
    repair_backlog,
    txn_orphans,
);

impl Stats {
    pub(crate) fn record_update(&self, pool_of: Option<DexKind>, applied: Applied) {
        match applied {
            Applied::Stale => {
                self.stale.fetch_add(1, Ordering::Relaxed);
                return;
            }
            Applied::Late => {
                self.late.fetch_add(1, Ordering::Relaxed);
                return;
            }
            Applied::Overflowed => {
                self.overflowed.fetch_add(1, Ordering::Relaxed);
            }
            Applied::Stored | Applied::Committed => {}
        }
        let counter = pool_of.map_or(&self.dependency_updates, |d| &self.pool_updates[d as usize]);
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_slot(&self, slot: Slot) {
        self.last_slot.fetch_max(slot.0, Ordering::Relaxed);
    }

    pub(crate) fn record_confirmation(&self, slot: Slot, resolved: &Resolved, gap: bool) {
        self.confirmed_slot.fetch_max(slot.0, Ordering::Relaxed);
        self.rolled_back
            .fetch_add(resolved.rolled_back as u64, Ordering::Relaxed);
        if gap {
            self.fork_gaps.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn add(&self, counter: Counter, n: usize) {
        self.counter(counter).fetch_add(n as u64, Ordering::Relaxed);
    }

    pub(crate) fn set(&self, counter: Counter, n: usize) {
        self.counter(counter).store(n as u64, Ordering::Relaxed);
    }

    const fn counter(&self, counter: Counter) -> &AtomicU64 {
        match counter {
            Counter::DeadDropped => &self.dead_dropped,
            Counter::Downs => &self.downs,
            Counter::Resumed => &self.resumed,
            Counter::Gaps => &self.gaps,
            Counter::GapKeys => &self.gap_keys,
            Counter::Rejected => &self.rejected,
            Counter::SeedsSent => &self.seeds_sent,
            Counter::SeedsFailed => &self.seeds_failed,
            Counter::AccountsSeeded => &self.accounts_seeded,
            Counter::AuditChecked => &self.audit_checked,
            Counter::AuditMismatches => &self.audit_mismatches,
            Counter::Keys => &self.keys,
            Counter::PoolsReady => &self.pools_ready,
            Counter::PoolsNotReady => &self.pools_not_ready,
            Counter::RepairBacklog => &self.repair_backlog,
            Counter::TxnOrphans => &self.txn_orphans,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Counter {
    DeadDropped,
    Downs,
    Resumed,
    Gaps,
    GapKeys,
    Rejected,
    SeedsSent,
    SeedsFailed,
    AccountsSeeded,
    AuditChecked,
    AuditMismatches,
    Keys,
    PoolsReady,
    PoolsNotReady,
    RepairBacklog,
    TxnOrphans,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dex_index_matches_position_in_all() {
        for (i, dex) in DexKind::ALL.into_iter().enumerate() {
            assert_eq!(dex as usize, i);
        }
    }

    #[test]
    fn merged_partitions_add_counters_and_keep_the_newest_slot() {
        let part = |stale, slot| {
            let stats = Stats::default();
            stats.stale.store(stale, Ordering::Relaxed);
            stats.last_slot.store(slot, Ordering::Relaxed);
            stats.snapshot()
        };
        let merged = StatsSnapshot::merge(&[part(2, 100), part(3, 90)]);
        assert_eq!((merged.stale, merged.last_slot), (5, 100));
    }

    #[test]
    fn stale_updates_are_not_counted_as_applied() {
        let stats = Stats::default();
        stats.record_update(Some(DexKind::RaydiumClmm), Applied::Stale);
        let snapshot = stats.snapshot();
        assert_eq!((snapshot.stale, snapshot.pool_updates[1].1), (1, 0));
    }
}
