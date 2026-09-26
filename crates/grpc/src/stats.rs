use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use domain::{LatencyHistogram, LatencySnapshot};

#[derive(Default)]
pub(crate) struct Stats {
    pub(crate) accounts: AtomicU64,
    pub(crate) statuses: AtomicU64,
    pub(crate) slots: AtomicU64,
    pub(crate) lag: LatencyHistogram,
    pub(crate) blocked: LatencyHistogram,
    pub(crate) queued_peak: AtomicU64,
}

#[derive(Clone, Default)]
pub struct GrpcStats(pub(crate) Arc<Stats>);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GrpcStatsSnapshot {
    pub accounts: u64,
    pub statuses: u64,
    pub slots: u64,
    /// From the plugin first seeing an account or block update to its
    /// arrival here.
    pub lag: LatencySnapshot,
    /// Time a stream waited to hand an event to its partition.
    pub blocked: LatencySnapshot,
    /// Most events waiting in one partition queue since the last snapshot.
    pub queued_peak: u64,
}

impl GrpcStats {
    #[must_use]
    pub fn snapshot(&self) -> GrpcStatsSnapshot {
        let stats = &self.0;
        let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
        GrpcStatsSnapshot {
            accounts: load(&stats.accounts),
            statuses: load(&stats.statuses),
            slots: load(&stats.slots),
            lag: stats.lag.take_interval(),
            blocked: stats.blocked.take_interval(),
            queued_peak: stats.queued_peak.swap(0, Ordering::Relaxed),
        }
    }
}
