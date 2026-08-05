use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

pub type ThreadPoolHandle = std::sync::mpsc::Sender<Box<dyn FnOnce() + Send>>;

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
