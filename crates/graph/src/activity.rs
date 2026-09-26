use std::sync::atomic::{AtomicU64, Ordering};

use crate::PoolId;

/// Which pools may be quoted right now, one bit per pool. Each pool has a
/// single writer (the pipeline thread of its partition); bits sharing a word are
/// set with atomic or/and, so writers never lose each other's flips.
pub struct Activity {
    words: Box<[AtomicU64]>,
    flips: AtomicU64,
}

impl Activity {
    pub(crate) fn new(pools: usize) -> Self {
        Self {
            words: (0..pools.div_ceil(64)).map(|_| AtomicU64::new(0)).collect(),
            flips: AtomicU64::new(0),
        }
    }

    pub fn set(&self, pool: PoolId, active: bool) {
        let (word, mask) = locate(pool);
        let before = if active {
            self.words[word].fetch_or(mask, Ordering::Relaxed)
        } else {
            self.words[word].fetch_and(!mask, Ordering::Relaxed)
        };
        if (before & mask != 0) != active {
            self.flips.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[must_use]
    pub fn is_active(&self, pool: PoolId) -> bool {
        let (word, mask) = locate(pool);
        self.words[word].load(Ordering::Relaxed) & mask != 0
    }

    #[must_use]
    pub fn active(&self) -> usize {
        self.words
            .iter()
            .map(|w| w.load(Ordering::Relaxed).count_ones() as usize)
            .sum()
    }

    #[must_use]
    pub fn flips(&self) -> u64 {
        self.flips.load(Ordering::Relaxed)
    }
}

fn locate(pool: PoolId) -> (usize, u64) {
    (pool.index() / 64, 1 << (pool.index() % 64))
}
