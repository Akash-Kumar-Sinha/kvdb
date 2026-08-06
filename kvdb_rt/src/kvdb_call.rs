use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

/// The sending half of a worker-thread pool's job queue: a channel of boxed,
/// one-shot closures.
pub type ThreadPoolHandle = std::sync::mpsc::Sender<Box<dyn FnOnce() + Send>>;

/// A hand-rolled [`Future`] that dispatches a blocking closure to a worker
/// thread on first poll and completes when that thread finishes.
///
/// This crate ships no executor and no [`std::task::Waker`] of its own — it
/// composes with whatever `Future`-polling executor the caller already has
/// (`tokio`, `async-std`, or a hand-rolled `block_on` in tests). The thread
/// that wakes the task is never the thread that polled it, since the worker
/// thread calls `Waker::wake` after running the job.
pub struct KvdbCall<R> {
    dispatched: bool,
    result: Arc<Mutex<Option<R>>>,
    job: Option<Box<dyn FnOnce() -> R + Send>>,
    pool: ThreadPoolHandle,
}

impl<R> KvdbCall<R>
where
    R: Send + 'static,
{
    /// Wraps `job` in a future that, once polled, dispatches it to `pool` and
    /// completes with its return value.
    pub fn new(job: Box<dyn FnOnce() -> R + Send>, pool: ThreadPoolHandle) -> Self {
        KvdbCall {
            dispatched: false,
            result: Arc::new(Mutex::new(None)),
            job: Some(job),
            pool,
        }
    }
}

impl<R> Future for KvdbCall<R>
where
    R: Send + 'static,
{
    type Output = R;

    /// Dispatches the job to the pool on the first poll and returns
    /// `Pending`; on later polls, returns `Ready` once the worker thread has
    /// written a result and called `Waker::wake`.
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<R> {
        let this = self.get_mut();

        if !this.dispatched {
            this.dispatched = true;

            let job = this.job.take().expect("job already dispatched");
            let result = Arc::clone(&this.result);
            let waker = cx.waker().clone();

            this.pool
                .send(Box::new(move || {
                    let mut result = result.lock().expect("result mutex not poisoned");
                    *result = Some(job());
                    waker.wake();
                }))
                .expect("thread pool sender closed");

            return Poll::Pending;
        }

        match this
            .result
            .lock()
            .expect("result mutex not poisoned")
            .take()
        {
            Some(value) => Poll::Ready(value),
            None => Poll::Pending,
        }
    }
}
