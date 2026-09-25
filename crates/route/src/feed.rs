use std::sync::Arc;

use domain::{ChainClock, DexKind, Pubkey};
use market::{MarketReader, PoolChanged, PoolView, Readiness};
use tokio::sync::broadcast;

pub trait PoolFeed: Clone + Send + Sync + 'static {
    fn pools(&self) -> Vec<(Pubkey, DexKind, Readiness)>;
    fn pool_view(&self, pool: &Pubkey) -> Option<Arc<PoolView>>;
    fn clock(&self) -> Option<ChainClock>;
    fn subscribe(&self) -> broadcast::Receiver<PoolChanged>;
}

impl PoolFeed for MarketReader {
    fn pools(&self) -> Vec<(Pubkey, DexKind, Readiness)> {
        Self::pools(self)
    }

    fn pool_view(&self, pool: &Pubkey) -> Option<Arc<PoolView>> {
        Self::pool_view(self, pool)
    }

    fn clock(&self) -> Option<ChainClock> {
        Self::clock(self)
    }

    fn subscribe(&self) -> broadcast::Receiver<PoolChanged> {
        Self::subscribe(self)
    }
}
