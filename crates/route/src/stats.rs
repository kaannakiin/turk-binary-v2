use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
pub(crate) struct Stats {
    pub decoded: AtomicU64,
    pub decode_errors: AtomicU64,
    pub panics: AtomicU64,
    pub lagged: AtomicU64,
    pub pools: AtomicU64,
    pub quotable: AtomicU64,
    pub unsupported: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RouteStatsSnapshot {
    pub decoded: u64,
    pub decode_errors: u64,
    pub panics: u64,
    /// Times a thread fell behind the change feed and rescanned.
    pub lagged: u64,
    pub pools: u64,
    /// Ready pools whose every account decoded.
    pub quotable: u64,
    pub unsupported: u64,
}

impl Stats {
    pub(crate) fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    pub(crate) fn set(counter: &AtomicU64, n: usize) {
        counter.store(u64::try_from(n).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> RouteStatsSnapshot {
        let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
        RouteStatsSnapshot {
            decoded: load(&self.decoded),
            decode_errors: load(&self.decode_errors),
            panics: load(&self.panics),
            lagged: load(&self.lagged),
            pools: load(&self.pools),
            quotable: load(&self.quotable),
            unsupported: load(&self.unsupported),
        }
    }
}

impl RouteStatsSnapshot {
    pub(crate) fn merge(parts: &[Self]) -> Self {
        parts.iter().fold(Self::default(), |a, b| Self {
            decoded: a.decoded + b.decoded,
            decode_errors: a.decode_errors + b.decode_errors,
            panics: a.panics + b.panics,
            lagged: a.lagged + b.lagged,
            pools: a.pools + b.pools,
            quotable: a.quotable + b.quotable,
            unsupported: a.unsupported + b.unsupported,
        })
    }
}
