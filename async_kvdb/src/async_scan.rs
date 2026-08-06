use btree::{DbError, Node, PagerState, Value};
use kvdb_rt::{KvdbCall, ThreadPoolHandle};
use scan::{Cursor, Step, step};
use serde::{Serialize, de::DeserializeOwned};
use spinlock::SpinLock;
use std::{
    future::Future,
    marker::PhantomData,
    mem,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

type Item<S> = Result<(S, Value), DbError>;
type Advanced<S> = (Cursor, Option<Item<S>>);
type AdvanceJob<S> = Box<dyn FnOnce() -> Advanced<S> + Send>;

/// An async cursor over a tree's entries, sorted by key.
///
/// The async counterpart to `scan::ScanIter`. Where the sync cursor lends out
/// borrowed `(&S, &Value)` pairs, this one hands back owned `(S, Value)` via
/// [`AsyncScanIter::next`], since each step is dispatched to a worker thread
/// through a job closure that must be `Send + 'static` — a borrow into this
/// iterator cannot satisfy that. `next` is an inherent method rather than a
/// trait method, so no `LendingIterator`-style import is needed to drive it.
pub struct AsyncScanIter<S> {
    pager_state: Arc<SpinLock<PagerState>>,
    cursor: Cursor,
    pool: ThreadPoolHandle,
    _marker: PhantomData<S>,
}

impl<S> AsyncScanIter<S> {
    pub(crate) fn new(
        pager_state: Arc<SpinLock<PagerState>>,
        cursor: Cursor,
        pool: ThreadPoolHandle,
    ) -> Self {
        AsyncScanIter {
            pager_state,
            cursor,
            pool,
            _marker: PhantomData,
        }
    }
}

fn advance<S>(pager_state: &Arc<SpinLock<PagerState>>, cursor: &mut Cursor) -> Option<Item<S>>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    loop {
        let (page_id, idx) = (*cursor)?;
        let node: Node<S> = match pager_state.acquire().pager.read_page(page_id) {
            Ok(node) => node,
            Err(err) => {
                *cursor = None;
                return Some(Err(err));
            }
        };

        match step(&node, idx) {
            Step::Yield(i) => {
                *cursor = Some((page_id, idx + 1));
                return Some(Ok((node.keys[i].clone(), node.values[i].clone())));
            }
            Step::Goto(target) => *cursor = Some((target, 0)),
            Step::Stop => *cursor = None,
        }
    }
}

/// The future returned by [`AsyncScanIter::next`].
///
/// Awaits to `None` once the scan is exhausted, or `Some(Err(_))` if a page
/// could not be read, mirroring `LendingIterator::Item` for the sync cursor.
pub struct NextCall<'a, S> {
    inner: KvdbCall<Advanced<S>>,
    cursor_slot: &'a mut Cursor,
}

impl<S> Future for NextCall<'_, S>
where
    S: Send + 'static,
{
    type Output = Option<Item<S>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll(cx) {
            Poll::Ready((cursor, item)) => {
                *this.cursor_slot = cursor;
                Poll::Ready(item)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> AsyncScanIter<S>
where
    S: Ord + Clone + Serialize + DeserializeOwned + Send + 'static,
{
    /// Advances the cursor by one step and returns a future for the next
    /// entry, or `None` once the scan is exhausted.
    ///
    /// Each call dispatches one traversal step to the worker pool via the
    /// same shared `scan::step` function the sync `ScanIter` uses, so the two
    /// walkers cannot disagree about ordering.
    pub fn next(&mut self) -> NextCall<'_, S> {
        let pager_state = Arc::clone(&self.pager_state);
        let mut cursor = mem::take(&mut self.cursor);

        let job: AdvanceJob<S> = Box::new(move || {
            let item = advance::<S>(&pager_state, &mut cursor);
            (cursor, item)
        });

        NextCall {
            inner: KvdbCall::new(job, self.pool.clone()),
            cursor_slot: &mut self.cursor,
        }
    }
}
