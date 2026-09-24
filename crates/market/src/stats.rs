use std::sync::atomic::{AtomicU64, Ordering};

use domain::{DexKind, Slot};

use crate::store::Resolved;

#[derive(Debug, Default)]
pub struct Stats {
    per_dex: [AtomicU64; DexKind::ALL.len()],
    other: AtomicU64,
    stale: AtomicU64,
    last_slot: AtomicU64,
    confirmed_slot: AtomicU64,
    rolled_back: AtomicU64,
    dead_dropped: AtomicU64,
    fork_gaps: AtomicU64,
    reconnects: AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsSnapshot {
    pub per_dex: Vec<(DexKind, u64)>,
    pub other: u64,
    pub stale: u64,
    pub last_slot: Slot,
    pub confirmed_slot: Slot,
    pub rolled_back: u64,
    pub dead_dropped: u64,
    /// Confirmations whose parent chain had a hole; versions below it were
    /// promoted without a fork check.
    pub fork_gaps: u64,
    pub reconnects: u64,
}

impl Stats {
    pub(crate) fn record_update(&self, dex: Option<DexKind>, applied: bool) {
        if !applied {
            self.stale.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let counter = dex.map_or(&self.other, |d| &self.per_dex[d as usize]);
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn record_slot(&self, slot: Slot) {
        self.last_slot.fetch_max(slot.0, Ordering::Relaxed);
    }

    pub(crate) fn record_confirmation(&self, slot: Slot, resolved: Resolved, gap: bool) {
        self.confirmed_slot.fetch_max(slot.0, Ordering::Relaxed);
        self.rolled_back
            .fetch_add(resolved.rolled_back as u64, Ordering::Relaxed);
        if gap {
            self.fork_gaps.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_dead(&self, dropped: usize) {
        self.dead_dropped
            .fetch_add(dropped as u64, Ordering::Relaxed);
    }

    pub(crate) fn record_reconnect(&self) {
        self.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    #[must_use]
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            per_dex: DexKind::ALL
                .into_iter()
                .map(|d| (d, self.per_dex[d as usize].load(Ordering::Relaxed)))
                .collect(),
            other: self.other.load(Ordering::Relaxed),
            stale: self.stale.load(Ordering::Relaxed),
            last_slot: Slot(self.last_slot.load(Ordering::Relaxed)),
            confirmed_slot: Slot(self.confirmed_slot.load(Ordering::Relaxed)),
            rolled_back: self.rolled_back.load(Ordering::Relaxed),
            dead_dropped: self.dead_dropped.load(Ordering::Relaxed),
            fork_gaps: self.fork_gaps.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
        }
    }
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
}
