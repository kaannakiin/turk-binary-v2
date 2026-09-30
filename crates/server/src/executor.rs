use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use crossbeam_channel::{Receiver, Sender};
use tokio::sync::{Notify, oneshot};
use tokio::time::Instant;

use crate::error::ServerError;

type Job = Box<dyn FnOnce() + Send>;

/// Searches run here, off the async runtime: at most one per thread, and at
/// most `max_queued` more wait. Past that a submit is refused at once, so a
/// burst gets `OVERLOADED` instead of a queue that grows without bound.
pub struct SearchPool {
    jobs: Sender<Job>,
    shared: Arc<Shared>,
    capacity: usize,
}

#[derive(Default)]
struct Shared {
    admitted: AtomicUsize,
    closed: AtomicBool,
    idle: Notify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    Full,
    Closed,
}

impl SearchPool {
    pub fn start(threads: usize, max_queued: usize) -> Result<Self, ServerError> {
        let threads = threads.max(1);
        let (jobs, queue) = crossbeam_channel::unbounded::<Job>();
        for i in 0..threads {
            let queue = queue.clone();
            thread::Builder::new()
                .name(format!("search-{i}"))
                .spawn(move || work(&queue))
                .map_err(ServerError::Spawn)?;
        }
        Ok(Self {
            jobs,
            shared: Arc::default(),
            capacity: threads.saturating_add(max_queued),
        })
    }

    /// A job holds its place until it returns, even after the caller stopped
    /// waiting: running work must observe its own cooperative cancellation.
    /// One whose caller left before it started is dropped unrun.
    pub(crate) fn submit<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<oneshot::Receiver<T>, Refused> {
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(Refused::Closed);
        }
        let admission = Admission::take(&self.shared, self.capacity).ok_or(Refused::Full)?;
        let (reply, result) = oneshot::channel();
        let job: Job = Box::new(move || {
            if reply.is_closed() {
                return;
            }
            let result = catch_unwind(AssertUnwindSafe(work));
            // Freed before the reply, so a caller that saw its answer finds
            // the place open again.
            drop(admission);
            if let Ok(value) = result {
                let _ = reply.send(value);
            } else {
                tracing::error!("a search panicked");
            }
        });
        self.jobs.send(job).map_err(|_| Refused::Closed)?;
        Ok(result)
    }

    /// Refuses new searches, then waits until every admitted one has
    /// returned or been skipped, up to `deadline`. `false`: some were still
    /// running, and keep running on state nothing updates any more.
    pub(crate) async fn close(&self, deadline: Instant) -> bool {
        self.shared.closed.store(true, Ordering::Release);
        loop {
            let idle = self.shared.idle.notified();
            tokio::pin!(idle);
            // Registered before the count is read, so a release in between
            // still wakes this wait.
            idle.as_mut().enable();
            if self.shared.admitted.load(Ordering::Acquire) == 0 {
                return true;
            }
            if tokio::time::timeout_at(deadline, idle).await.is_err() {
                return false;
            }
        }
    }
}

fn work(queue: &Receiver<Job>) {
    for job in queue {
        job();
    }
}

struct Admission(Arc<Shared>);

impl Admission {
    fn take(shared: &Arc<Shared>, capacity: usize) -> Option<Self> {
        shared
            .admitted
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < capacity).then_some(n + 1)
            })
            .ok()?;
        Some(Self(Arc::clone(shared)))
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if self.0.admitted.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}
