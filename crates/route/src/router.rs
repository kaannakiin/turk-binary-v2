use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use domain::LatencyHistogram;
use graph::Topology;
use serde::Deserialize;
use tokio::sync::{oneshot, watch};

use crate::error::RouteError;
use crate::feed::PoolFeed;
use crate::reader::{QuoteReader, Table};
use crate::stats::{RouteStatsSnapshot, Stats};
use crate::worker::Worker;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RouteSettings {
    pub route_threads: u16,
    pub probe_amount: u64,
}

impl Default for RouteSettings {
    fn default() -> Self {
        Self {
            route_threads: 4,
            probe_amount: 1_000_000,
        }
    }
}

impl RouteSettings {
    pub fn threads(&self) -> Result<usize, RouteError> {
        match self.route_threads {
            0 => Err(RouteError::Threads),
            n => Ok(usize::from(n)),
        }
    }
}

/// `route_threads` OS threads, each owning the pools its hash picks: it
/// decodes their accounts as the market changes them and publishes the state
/// for [`QuoteReader`].
pub struct Router<F> {
    reader: QuoteReader<F>,
    stats: Vec<Arc<Stats>>,
    batch: Arc<LatencyHistogram>,
    stop: watch::Sender<bool>,
    stopped: Vec<oneshot::Receiver<Result<(), RouteError>>>,
}

impl<F: PoolFeed> Router<F> {
    pub fn start(
        feed: F,
        settings: &RouteSettings,
        topology: &Arc<Topology>,
    ) -> Result<Self, RouteError> {
        let threads = settings.threads()?;
        let table = Arc::new(Table::default());
        let batch = Arc::new(LatencyHistogram::default());
        let (stop, stop_rx) = watch::channel(false);
        let mut stats = Vec::with_capacity(threads);
        let mut stopped = Vec::with_capacity(threads);
        for index in 0..threads {
            let thread_stats = Arc::new(Stats::default());
            stats.push(Arc::clone(&thread_stats));
            let worker = Worker::new(
                index,
                threads,
                feed.clone(),
                Arc::clone(&table),
                thread_stats,
                Arc::clone(topology),
                Arc::clone(&batch),
            );
            let stop_rx = stop_rx.clone();
            let (done, done_rx) = oneshot::channel();
            std::thread::Builder::new()
                .name(format!("route-r{index}"))
                .spawn(move || {
                    let result = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(RouteError::Thread)
                        .map(|runtime| runtime.block_on(worker.run(stop_rx)));
                    let _ = done.send(result);
                })
                .map_err(RouteError::Thread)?;
            stopped.push(done_rx);
        }
        Ok(Self {
            reader: QuoteReader { feed, table },
            stats,
            batch,
            stop,
            stopped,
        })
    }

    #[must_use]
    pub fn reader(&self) -> QuoteReader<F> {
        self.reader.clone()
    }

    #[must_use]
    pub fn stats(&self) -> RouteStatsSnapshot {
        let parts: Vec<RouteStatsSnapshot> = self.stats.iter().map(|s| s.snapshot()).collect();
        RouteStatsSnapshot {
            batch: self.batch.snapshot(),
            ..RouteStatsSnapshot::merge(&parts)
        }
    }

    pub fn shutdown(&self) {
        let _ = self.stop.send(true);
    }

    /// Resolves when a thread stops, with its result. Dropping the future
    /// loses nothing, so it can sit in a `select!` loop.
    pub async fn stopped(&mut self) -> Result<(), RouteError> {
        std::future::poll_fn(|cx| {
            let ready = self.stopped.iter_mut().enumerate().find_map(|(i, stop)| {
                match Pin::new(stop).poll(cx) {
                    Poll::Ready(result) => Some((i, result)),
                    Poll::Pending => None,
                }
            });
            ready.map_or(Poll::Pending, |(i, result)| {
                self.stopped.swap_remove(i);
                Poll::Ready(result.unwrap_or(Err(RouteError::ThreadPanicked)))
            })
        })
        .await
    }
}
