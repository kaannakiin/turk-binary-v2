use std::sync::Arc;

use domain::{ChainClock, Pubkey};
use market::{MarketReader, PoolView};

/// The market's read side, as quotes use it.
pub trait PoolFeed: Clone + Send + Sync + 'static {
    fn pool_view(&self, pool: &Pubkey) -> Option<Arc<PoolView>>;
    fn clock(&self) -> Option<ChainClock>;
}

impl PoolFeed for MarketReader {
    fn pool_view(&self, pool: &Pubkey) -> Option<Arc<PoolView>> {
        Self::pool_view(self, pool)
    }

    fn clock(&self) -> Option<ChainClock> {
        Self::clock(self)
    }
}
