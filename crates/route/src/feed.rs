use domain::ChainClock;
use market::MarketReader;

/// The market's read side, as quotes use it.
pub trait PoolFeed: Clone + Send + Sync + 'static {
    fn clock(&self) -> Option<ChainClock>;
}

impl PoolFeed for MarketReader {
    fn clock(&self) -> Option<ChainClock> {
        Self::clock(self)
    }
}
