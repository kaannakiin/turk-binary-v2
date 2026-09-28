use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::SearchPool;
use crate::executor::Overloaded;

/// A job that holds its thread until the returned sender is dropped. Keep
/// the receiver too: a job whose receiver is gone is skipped.
fn blocker(pool: &SearchPool) -> (mpsc::Sender<()>, tokio::sync::oneshot::Receiver<()>) {
    let (release, gate) = mpsc::channel::<()>();
    let done = pool
        .submit(move || {
            let _ = gate.recv();
        })
        .expect("admitted");
    (release, done)
}

#[tokio::test]
async fn a_submit_past_threads_plus_queue_is_refused() {
    let pool = SearchPool::start(1, 1).expect("starts");
    let (_running, _running_done) = blocker(&pool);
    let (_queued, _queued_done) = blocker(&pool);

    let refused = pool.submit(|| ());

    assert_eq!(refused.err(), Some(Overloaded));
}

#[tokio::test]
async fn a_finished_job_frees_its_place() {
    let pool = SearchPool::start(1, 0).expect("starts");
    let (release, done) = blocker(&pool);
    drop(release);
    done.await.expect("the job ran");

    let admitted = pool.submit(|| 7).expect("admitted").await;

    assert_eq!(admitted, Ok(7));
}

#[tokio::test]
async fn a_job_whose_caller_left_before_it_started_is_not_run() {
    let pool = SearchPool::start(1, 2).expect("starts");
    let (release, _blocker_done) = blocker(&pool);
    let ran = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ran);
    let abandoned = pool
        .submit(move || flag.store(true, Ordering::SeqCst))
        .expect("admitted");

    drop(abandoned);
    drop(release);
    // One thread takes jobs in order, so this one runs after the abandoned one.
    pool.submit(|| ()).expect("admitted").await.expect("ran");

    assert!(!ran.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_panicking_job_leaves_its_thread_serving() {
    let pool = SearchPool::start(1, 0).expect("starts");
    let panicked = pool
        .submit(|| panic!("search blew up"))
        .expect("admitted")
        .await;

    let next = pool.submit(|| 7).expect("admitted").await;

    assert!(panicked.is_err());
    assert_eq!(next, Ok(7));
}
