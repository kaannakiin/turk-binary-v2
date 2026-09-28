use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use arc_swap::{ArcSwap, ArcSwapOption};
use dex::Dependency;
use domain::{ChainClock, DexKind, Pubkey, Slot};
use tokio::sync::broadcast;

use crate::readiness::Readiness;
use crate::store::StoredAccount;

#[derive(Debug, Clone)]
pub(crate) struct PoolMeta {
    pub dex: DexKind,
    pub deps: Arc<[Dependency]>,
    pub readiness: Readiness,
    pub cross_stream: bool,
}

pub(crate) type PoolTable = HashMap<Pubkey, PoolMeta>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolChanged {
    pub pool: Pubkey,
    pub slot: Slot,
}

/// One pool as the engine last published it. The Clock is not in
/// `accounts`; read it with [`MarketReader::clock`].
#[derive(Debug, Clone)]
pub struct PoolView {
    pub pool: Pubkey,
    pub dex: DexKind,
    pub readiness: Readiness,
    /// `None`: not known yet. An account with `lamports == 0` is confirmed
    /// absent (an optional array that does not exist).
    pub accounts: Vec<(Dependency, Option<StoredAccount>)>,
    /// Some account a swap writes rides the shared stream, so this view may
    /// hold part of a transaction.
    pub cross_stream: bool,
}

impl PoolView {
    #[must_use]
    pub fn get(&self, role: dex::Role) -> Option<&StoredAccount> {
        self.accounts
            .iter()
            .find(|(d, _)| d.role == role)
            .and_then(|(_, a)| a.as_ref())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolCounts {
    pub ready: usize,
    /// Ready, or not ready for a reason that clears by itself.
    pub eligible: usize,
    pub total: usize,
}

type Cells = HashMap<Pubkey, Arc<ArcSwap<PoolView>>, ahash::RandomState>;

/// The pool set changes only with the universe, so the map is swapped
/// whole on those rare changes, and each pool's own cell on every update.
#[derive(Default)]
pub(crate) struct Snapshots {
    pools: ArcSwap<Cells>,
    clock: ArcSwapOption<ClockCell>,
}

#[derive(Debug, Clone, Copy)]
struct ClockCell {
    clock: ChainClock,
    advanced_at: Instant,
}

impl Snapshots {
    pub(crate) fn publish(&self, view: Arc<PoolView>) {
        if let Some(cell) = self.pools.load().get(&view.pool) {
            cell.store(view);
            return;
        }
        let pool = view.pool;
        let cell = Arc::new(ArcSwap::new(view));
        self.pools.rcu(|pools| {
            let mut pools = Cells::clone(pools);
            pools.insert(pool, Arc::clone(&cell));
            pools
        });
    }

    /// Every partition writes its own stream's Clock into this one cell, and a
    /// rolled-back fork moves one partition's Clock back; only a newer slot
    /// replaces the cell, and only it moves `advanced_at`.
    pub(crate) fn set_clock(&self, clock: Option<ChainClock>) {
        let Some(clock) = clock else {
            return;
        };
        let now = Instant::now();
        self.clock.rcu(|current| match current {
            Some(current) if current.clock.slot >= clock.slot => Some(Arc::clone(current)),
            _ => Some(Arc::new(ClockCell {
                clock,
                advanced_at: now,
            })),
        });
    }
}

/// Read side for the quote layer. Reads never take a lock and never wait on
/// the engine.
#[derive(Clone)]
pub struct MarketReader {
    pub(crate) snapshots: Arc<Snapshots>,
    pub(crate) changes: broadcast::Sender<PoolChanged>,
}

impl MarketReader {
    #[must_use]
    pub fn pools(&self) -> Vec<(Pubkey, DexKind, Readiness)> {
        self.snapshots
            .pools
            .load()
            .values()
            .map(|cell| {
                let view = cell.load();
                (view.pool, view.dex, view.readiness)
            })
            .collect()
    }

    #[must_use]
    pub fn pool_counts(&self) -> PoolCounts {
        let cells = self.snapshots.pools.load();
        let mut counts = PoolCounts {
            ready: 0,
            eligible: 0,
            total: cells.len(),
        };
        for cell in cells.values() {
            match cell.load().readiness {
                Readiness::Ready => {
                    counts.ready += 1;
                    counts.eligible += 1;
                }
                Readiness::NotReady(reason) if !reason.is_permanent() => counts.eligible += 1,
                Readiness::NotReady(_) => {}
            }
        }
        counts
    }

    #[must_use]
    pub fn readiness(&self, pool: &Pubkey) -> Option<Readiness> {
        self.pool_view(pool).map(|view| view.readiness)
    }

    #[must_use]
    pub fn pool_view(&self, pool: &Pubkey) -> Option<Arc<PoolView>> {
        self.snapshots
            .pools
            .load()
            .get(pool)
            .map(|cell| cell.load_full())
    }

    /// From the Clock sysvar, never the host clock: fees and activation use
    /// the chain's notion of time.
    #[must_use]
    pub fn clock(&self) -> Option<ChainClock> {
        self.snapshots
            .clock
            .load()
            .as_deref()
            .map(|cell| cell.clock)
    }

    /// Host time when the Clock's slot last moved. The Clock rides every
    /// stream, so a feed whose Clock stopped has stopped as a whole, however
    /// ready its pools still look.
    #[must_use]
    pub fn clock_advanced_at(&self) -> Option<Instant> {
        self.snapshots
            .clock
            .load()
            .as_deref()
            .map(|cell| cell.advanced_at)
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<PoolChanged> {
        self.changes.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use domain::{ChainClock, Slot};

    use super::Snapshots;

    fn clock(slot: u64) -> ChainClock {
        ChainClock {
            slot: Slot(slot),
            epoch_start_timestamp: 0,
            epoch: 0,
            leader_schedule_epoch: 0,
            unix_timestamp: i64::try_from(slot).expect("small slot"),
        }
    }

    #[test]
    fn a_partition_behind_another_does_not_move_the_clock_or_its_freshness_back() {
        let snapshots = Snapshots::default();
        snapshots.set_clock(Some(clock(200)));
        let advanced = snapshots
            .clock
            .load()
            .as_deref()
            .map(|cell| cell.advanced_at);
        snapshots.set_clock(Some(clock(199)));
        snapshots.set_clock(None);
        let cell = snapshots.clock.load().as_deref().copied();
        assert_eq!(cell.map(|cell| cell.clock), Some(clock(200)));
        assert_eq!(cell.map(|cell| cell.advanced_at), advanced);
    }
}
