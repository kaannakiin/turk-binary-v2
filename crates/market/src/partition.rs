use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use grpc::{GroupKey, Partition};
use tokio::sync::{broadcast, oneshot};

use crate::engine::{Engine, SyncSettings};
use crate::ports::AccountSource;
use crate::stats::{Stats, StatsSnapshot, Timings, TimingsSnapshot};
use crate::view::{MarketReader, Snapshots};
use crate::{MarketError, Universe, ViewSink};

/// One engine per hub partition, each on its own thread, sharing nothing
/// but the read side: a pool's shard decides its partition, and every
/// partition has its own shared stream.
pub struct Market {
    reader: MarketReader,
    stats: Vec<Arc<Stats>>,
    timings: Arc<Timings>,
    stopped: Vec<oneshot::Receiver<Result<(), MarketError>>>,
}

/// How many pipeline threads (partitions) to run. `0` picks the largest
/// divisor of `streams` up to half the cores: shard `i` feeds partition
/// `i % partitions`, so a divisor gives every partition as many shards.
pub fn pipeline_threads(requested: u16, streams: u16, cores: usize) -> Result<u16, MarketError> {
    if requested == 0 {
        let cap = u16::try_from((cores / 2).max(1)).unwrap_or(u16::MAX);
        return Ok((1..=streams.min(cap))
            .rev()
            .find(|n| streams.is_multiple_of(*n))
            .unwrap_or(1));
    }
    if (1..=streams).contains(&requested) {
        Ok(requested)
    } else {
        Err(MarketError::Partitions { requested, streams })
    }
}

impl Market {
    pub fn start<S: AccountSource>(
        universe: &Universe,
        source: &Arc<S>,
        partitions: Vec<Partition>,
        settings: &SyncSettings,
        global_tree: bool,
        mut sinks: impl FnMut(usize) -> Box<dyn ViewSink>,
    ) -> Result<Self, MarketError> {
        let snapshots = Arc::new(Snapshots::default());
        let changes = broadcast::channel(4_096).0;
        let timings = Arc::new(Timings::default());
        let mut stats = Vec::new();
        let mut stopped = Vec::new();
        for (
            i,
            Partition {
                hub,
                mut events,
                streams,
            },
        ) in partitions.into_iter().enumerate()
        {
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
            engine.share_output(&snapshots, &changes, &timings);
            engine.set_sink(sinks(i));
            stats.push(engine.stats());
            let (done, stop) = oneshot::channel();
            std::thread::Builder::new()
                .name(format!("pipe-p{i}"))
                .spawn(move || {
                    let result = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(MarketError::Partition)
                        .and_then(|runtime| {
                            runtime.block_on(async move {
                                tokio::select! {
                                    result = engine.run(&mut events) => result,
                                    result = streams.run() => Err(result
                                        .err()
                                        .map_or(MarketError::StreamClosed, MarketError::Grpc)),
                                }
                            })
                        });
                    let _ = done.send(result);
                })
                .map_err(MarketError::Partition)?;
            stopped.push(stop);
        }
        Ok(Self {
            reader: MarketReader { snapshots, changes },
            stats,
            timings,
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

    /// Engine step times since the previous call, over every partition.
    #[must_use]
    pub fn timings(&self) -> TimingsSnapshot {
        self.timings.take_interval()
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
            timings: Arc::default(),
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
    fn pipeline_threads_divide_the_streams_when_automatic_and_must_fit_them_otherwise() {
        let cases = [
            ((0, 12, 8), Some(4)),
            ((0, 12, 10), Some(4)),
            ((0, 12, 64), Some(12)),
            ((0, 7, 8), Some(1)),
            ((0, 12, 1), Some(1)),
            ((5, 12, 8), Some(5)),
            ((12, 12, 8), Some(12)),
            ((13, 12, 8), None),
        ];
        for ((requested, streams, cores), expected) in cases {
            assert_eq!(
                pipeline_threads(requested, streams, cores).ok(),
                expected,
                "requested {requested}, {streams} streams, {cores} cores"
            );
        }
    }
}
