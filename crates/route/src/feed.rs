use std::time::Instant;

use domain::ChainClock;
use market::MarketReader;

/// The market's read side, as quotes use it.
pub trait PoolFeed: Clone + Send + Sync + 'static {
    fn clock(&self) -> Option<ChainClock>;

    /// Host time when the Clock's slot last moved: how stale the feed is.
    fn clock_advanced_at(&self) -> Option<Instant>;
}

impl PoolFeed for MarketReader {
    fn clock(&self) -> Option<ChainClock> {
        Self::clock(self)
    }

    fn clock_advanced_at(&self) -> Option<Instant> {
        Self::clock_advanced_at(self)
    }
}
