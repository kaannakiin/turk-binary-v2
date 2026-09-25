use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const SUB_BITS: u32 = 3;
const SUB: usize = 1 << SUB_BITS;
const GROUPS: usize = 64 - SUB_BITS as usize + 1;

/// Latencies in log-linear buckets: every power of two is split into eight,
/// so a reported quantile is at most 12.5% above the true one. Any number of
/// threads record at once without a lock.
pub struct LatencyHistogram {
    buckets: Box<[AtomicU64]>,
    max: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LatencySnapshot {
    pub count: u64,
    pub p50: Duration,
    pub p99: Duration,
    pub max: Duration,
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self {
            buckets: (0..GROUPS * SUB).map(|_| AtomicU64::new(0)).collect(),
            max: AtomicU64::new(0),
        }
    }
}

impl LatencyHistogram {
    pub fn record(&self, elapsed: Duration) {
        let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        self.buckets[index(nanos)].fetch_add(1, Ordering::Relaxed);
        self.max.fetch_max(nanos, Ordering::Relaxed);
    }

    #[must_use]
    pub fn snapshot(&self) -> LatencySnapshot {
        let counts: Vec<u64> = self
            .buckets
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .collect();
        let count = counts.iter().sum();
        let max = self.max.load(Ordering::Relaxed);
        let quantile =
            |per_mille: u64| Duration::from_nanos(quantile(&counts, count, per_mille).min(max));
        LatencySnapshot {
            count,
            p50: quantile(500),
            p99: quantile(990),
            max: Duration::from_nanos(max),
        }
    }
}

fn index(nanos: u64) -> usize {
    if nanos < SUB as u64 {
        return usize::try_from(nanos).unwrap_or(0);
    }
    let exponent = nanos.ilog2();
    let sub = (nanos >> (exponent - SUB_BITS)) & (SUB as u64 - 1);
    (exponent - SUB_BITS + 1) as usize * SUB + usize::try_from(sub).unwrap_or(0)
}

fn upper_bound(index: usize) -> u64 {
    let group = index / SUB;
    let sub = (index % SUB) as u64;
    if group == 0 {
        return sub;
    }
    let shift = u32::try_from(group - 1).unwrap_or(u32::MAX);
    let lower = (SUB as u64 + sub) << shift;
    lower.saturating_add((1u64 << shift) - 1)
}

fn quantile(counts: &[u64], count: u64, per_mille: u64) -> u64 {
    if count == 0 {
        return 0;
    }
    let rank = (u128::from(count) * u128::from(per_mille)).div_ceil(1_000);
    let mut seen = 0u128;
    for (i, n) in counts.iter().enumerate() {
        seen += u128::from(*n);
        if seen >= rank {
            return upper_bound(i);
        }
    }
    u64::MAX
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::LatencyHistogram;

    #[test]
    fn quantiles_stay_within_one_bucket_above_the_recorded_distribution() {
        let histogram = LatencyHistogram::default();
        for micros in 1..=1_000 {
            histogram.record(Duration::from_micros(micros));
        }
        let snapshot = histogram.snapshot();
        let within = |got: Duration, want: Duration| got >= want && got <= want + want / 8;
        assert_eq!(snapshot.count, 1_000);
        assert!(
            within(snapshot.p50, Duration::from_micros(500)),
            "{snapshot:?}"
        );
        assert!(
            within(snapshot.p99, Duration::from_micros(990)),
            "{snapshot:?}"
        );
        assert_eq!(snapshot.max, Duration::from_micros(1_000));
    }
}
