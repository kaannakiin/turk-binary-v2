use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use crossbeam_channel::{Receiver, Sender};
use tokio::sync::oneshot;

use crate::error::ServerError;

type Job = Box<dyn FnOnce() + Send>;

/// Searches run here, off the async runtime: at most one per thread, and at
/// most `max_queued` more wait. Past that a submit is refused at once, so a
/// burst gets `OVERLOADED` instead of a queue that grows without bound.
pub struct SearchPool {
    jobs: Sender<Job>,
    admitted: Arc<AtomicUsize>,
    capacity: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Overloaded;

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
            admitted: Arc::default(),
            capacity: threads.saturating_add(max_queued),
        })
    }

    /// A job holds its place until it returns, even after the caller stopped
    /// waiting: a running search cannot be interrupted. One whose caller left
    /// before it started is dropped unrun.
    pub(crate) fn submit<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<oneshot::Receiver<T>, Overloaded> {
        let admission = Admission::take(&self.admitted, self.capacity).ok_or(Overloaded)?;
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
        self.jobs.send(job).map_err(|_| Overloaded)?;
        Ok(result)
    }
}

fn work(queue: &Receiver<Job>) {
    for job in queue {
        job();
    }
}

struct Admission(Arc<AtomicUsize>);

impl Admission {
    fn take(admitted: &Arc<AtomicUsize>, capacity: usize) -> Option<Self> {
        admitted
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < capacity).then_some(n + 1)
            })
            .ok()?;
        Some(Self(Arc::clone(admitted)))
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
