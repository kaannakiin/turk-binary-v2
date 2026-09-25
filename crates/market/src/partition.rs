use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use grpc::{GroupKey, Partition};
use tokio::sync::{broadcast, oneshot};

use crate::engine::{Engine, SyncSettings};
use crate::ports::AccountSource;
use crate::stats::{Stats, StatsSnapshot};
use crate::view::{MarketReader, Snapshots};
use crate::{MarketError, Universe};

/// One engine per hub partition, each on its own thread, sharing nothing
/// but the read side: a pool's shard decides its partition, and every
/// partition has its own shared stream.
pub struct Market {
    reader: MarketReader,
    stats: Vec<Arc<Stats>>,
    stopped: Vec<oneshot::Receiver<Result<(), MarketError>>>,
}

impl Market {
    pub fn start<S: AccountSource>(
        universe: &Universe,
        source: &Arc<S>,
        partitions: Vec<Partition>,
        settings: &SyncSettings,
        global_tree: bool,
    ) -> Result<Self, MarketError> {
        let snapshots = Arc::new(Snapshots::default());
        let changes = broadcast::channel(4_096).0;
        let mut stats = Vec::new();
        let mut stopped = Vec::new();
        for (i, Partition { hub, mut events }) in partitions.into_iter().enumerate() {
            let owned = Universe {
                dexes: universe.dexes.clone(),
                pools: universe
                    .pools
                    .iter()
                    .filter(|(pool, _)| hub.owns(&GroupKey(**pool)))
                    .map(|(pool, info)| (*pool, info.clone()))
                    .collect(),
            };
            let mut engine = Engine::new(
                &owned,
                Arc::clone(source),
                hub,
                settings.clone(),
                global_tree,
            );
            engine.share_output(&snapshots, &changes);
            stats.push(engine.stats());
            let (done, stop) = oneshot::channel();
            std::thread::Builder::new()
                .name(format!("market-p{i}"))
                .spawn(move || {
                    let result = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(MarketError::Partition)
                        .and_then(|runtime| runtime.block_on(engine.run(&mut events)));
                    let _ = done.send(result);
                })
                .map_err(MarketError::Partition)?;
            stopped.push(stop);
        }
        Ok(Self {
            reader: MarketReader { snapshots, changes },
            stats,
            stopped,
        })
    }

    #[must_use]
    pub fn reader(&self) -> MarketReader {
        self.reader.clone()
    }

    #[must_use]
    pub fn stats(&self) -> StatsSnapshot {
        let parts: Vec<StatsSnapshot> = self.stats.iter().map(|s| s.snapshot()).collect();
        StatsSnapshot::merge(&parts)
    }

    /// Resolves when a partition stops, with its result. Dropping the
    /// future loses nothing, so it can sit in a `select!` loop.
    pub async fn stopped(&mut self) -> Result<(), MarketError> {
        std::future::poll_fn(|cx| {
            let ready = self.stopped.iter_mut().enumerate().find_map(|(i, stop)| {
                match Pin::new(stop).poll(cx) {
                    Poll::Ready(result) => Some((i, result)),
                    Poll::Pending => None,
                }
            });
            ready.map_or(Poll::Pending, |(i, result)| {
                self.stopped.swap_remove(i);
                Poll::Ready(result.unwrap_or(Err(MarketError::PartitionPanicked)))
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_abandoned_wait_does_not_lose_a_partitions_result() {
        let (done, stop) = oneshot::channel();
        let mut market = Market {
            reader: MarketReader {
                snapshots: Arc::default(),
                changes: broadcast::channel(1).0,
            },
            stats: Vec::new(),
            stopped: vec![stop],
        };
        let abandoned =
            tokio::time::timeout(std::time::Duration::from_millis(10), market.stopped()).await;
        assert!(abandoned.is_err());
        done.send(Err(MarketError::StreamClosed)).unwrap();
        assert!(matches!(
            market.stopped().await,
            Err(MarketError::StreamClosed)
        ));
    }

    #[test]
    fn pipeline_threads_defaults_to_two_and_must_fit_the_streams() {
        let with = |pipeline_threads| SyncSettings {
            pipeline_threads,
            ..SyncSettings::default()
        };
        assert_eq!(
            [
                SyncSettings::default().partitions(12).ok(),
                with(0).partitions(12).ok(),
                with(13).partitions(12).ok(),
                with(12).partitions(12).ok(),
            ],
            [Some(2), None, None, Some(12)]
        );
    }
}
