use btree::{DbError, Node, PageId, PagerState, Value};
use kvdb_rt::{KvdbCall, ThreadPoolHandle};
use scan::{Step, step};
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

type Cursor = Vec<(PageId, usize)>;
type Item<S> = Result<(S, Value), DbError>;
type Advanced<S> = (Cursor, Option<Item<S>>);
type AdvanceJob<S> = Box<dyn FnOnce() -> Advanced<S> + Send>;

pub struct AsyncScanIter<S> {
    pager_state: Arc<SpinLock<PagerState>>,
    stack: Cursor,
    pool: ThreadPoolHandle,
    _marker: PhantomData<S>,
}

impl<S> AsyncScanIter<S> {
    pub(crate) fn new(
        pager_state: Arc<SpinLock<PagerState>>,
        stack: Cursor,
        pool: ThreadPoolHandle,
    ) -> Self {
        AsyncScanIter {
            pager_state,
            stack,
            pool,
            _marker: PhantomData,
        }
    }
}

fn advance<S>(pager_state: &Arc<SpinLock<PagerState>>, stack: &mut Cursor) -> Option<Item<S>>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    loop {
        let (page_id, idx) = *stack.last()?;
        let node: Node<S> = match pager_state.acquire().pager.read_page(page_id) {
            Ok(node) => node,
            Err(err) => {
                stack.clear();
                return Some(Err(err));
            }
        };

        let cursor = |stack: &mut Cursor| {
            stack
                .last_mut()
                .expect("stack is non-empty while stepping")
                .1 += 1;
        };

        match step(&node, idx) {
            Step::Yield(i) => {
                cursor(stack);
                return Some(Ok((node.keys[i].clone(), node.values[i].clone())));
            }
            Step::Descend(child_id) => {
                cursor(stack);
                stack.push((child_id, 0));
            }
            Step::Pop => {
                stack.pop();
            }
        }
    }
}

pub struct NextCall<'a, S> {
    inner: KvdbCall<Advanced<S>>,
    stack_slot: &'a mut Cursor,
}

impl<S> Future for NextCall<'_, S>
where
    S: Send + 'static,
{
    type Output = Option<Item<S>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll(cx) {
            Poll::Ready((stack, item)) => {
                *this.stack_slot = stack;
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
    pub fn next(&mut self) -> NextCall<'_, S> {
        let pager_state = Arc::clone(&self.pager_state);
        let mut stack = mem::take(&mut self.stack);

        let job: AdvanceJob<S> = Box::new(move || {
            let item = advance::<S>(&pager_state, &mut stack);
            (stack, item)
        });

        NextCall {
            inner: KvdbCall::new(job, self.pool.clone()),
            stack_slot: &mut self.stack,
        }
    }
}
