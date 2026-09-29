use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;
use domain::chain::LatestBlockhash;

struct Fetched {
    blockhash: LatestBlockhash,
    at: Instant,
}

#[derive(Clone, Default)]
pub struct BlockhashSlot(Arc<ArcSwapOption<Fetched>>);

impl BlockhashSlot {
    pub fn set(&self, blockhash: LatestBlockhash) {
        self.0.store(Some(Arc::new(Fetched {
            blockhash,
            at: Instant::now(),
        })));
    }

    pub(crate) fn fresh(&self, max_age: Duration) -> Option<LatestBlockhash> {
        let fetched = self.0.load_full()?;
        (fetched.at.elapsed() <= max_age).then_some(fetched.blockhash)
    }

    /// Runs until the task is dropped. A failed fetch keeps the last
    /// blockhash, which ages out on its own.
    pub async fn refresh<F, Fut, E>(self, every: Duration, fetch: F)
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<LatestBlockhash, E>>,
        E: std::fmt::Display,
    {
        let mut ticks = tokio::time::interval(every);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            match fetch().await {
                Ok(blockhash) => self.set(blockhash),
                Err(error) => tracing::warn!(%error, "blockhash refresh failed"),
            }
        }
    }
}
