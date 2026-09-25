use std::collections::HashMap;
use std::sync::Arc;

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

type Cells = HashMap<Pubkey, Arc<ArcSwap<PoolView>>, ahash::RandomState>;

/// The pool set changes only with the universe, so the map is swapped
/// whole on those rare changes, and each pool's own cell on every update.
#[derive(Default)]
pub(crate) struct Snapshots {
    pools: ArcSwap<Cells>,
    clock: ArcSwapOption<ChainClock>,
}

impl Snapshots {
    pub(crate) fn publish(&self, view: PoolView) {
        if let Some(cell) = self.pools.load().get(&view.pool) {
            cell.store(Arc::new(view));
            return;
        }
        let pool = view.pool;
        let cell = Arc::new(ArcSwap::from_pointee(view));
        self.pools.rcu(|pools| {
            let mut pools = Cells::clone(pools);
            pools.insert(pool, Arc::clone(&cell));
            pools
        });
    }

    pub(crate) fn set_clock(&self, clock: Option<ChainClock>) {
        self.clock.store(clock.map(Arc::new));
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
        self.snapshots.clock.load().as_deref().copied()
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<PoolChanged> {
        self.changes.subscribe()
    }
}
