use std::sync::Arc;

use crate::view::PoolView;

/// Sees every view on the partition thread that publishes it, before any
/// reader can load it, so state derived here is never behind the market.
pub trait ViewSink: Send {
    fn publish(&mut self, view: &Arc<PoolView>);

    /// Once per engine tick, after readiness is refreshed.
    fn on_tick(&mut self) {}
}

pub(crate) struct NoSink;

impl ViewSink for NoSink {
    fn publish(&mut self, _: &Arc<PoolView>) {}
}
