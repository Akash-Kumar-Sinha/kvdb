use std::sync::Arc;

use btree::{BTree, DbError, Initialized, Node, PageId, PagerState, Value};
use serde::{Serialize, de::DeserializeOwned};
use spinlock::SpinLock;

pub trait LendingIterator {
    type Item<'a>
    where
        Self: 'a;
    fn next(&mut self) -> Option<Self::Item<'_>>;
}

pub type Cursor = Option<(PageId, usize)>;

pub enum Step {
    Yield(usize),
    Goto(PageId),
    Stop,
}

pub fn step<S>(node: &Node<S>, idx: usize) -> Step {
    if !node.is_leaf {
        return match node.children.first() {
            Some(&leftmost) => Step::Goto(leftmost),
            None => Step::Stop,
        };
    }

    if idx < node.keys.len() {
        Step::Yield(idx)
    } else {
        match node.next {
            Some(sibling) => Step::Goto(sibling),
            None => Step::Stop,
        }
    }
}

pub struct ScanIter<S> {
    pager_state: Arc<SpinLock<PagerState>>,
    cursor: Cursor,
    current: Option<Node<S>>,
}

impl<S> ScanIter<S> {
    pub fn new(pager_state: Arc<SpinLock<PagerState>>, root_id: PageId) -> Self {
        ScanIter {
            pager_state,
            cursor: Some((root_id, 0)),
            current: None,
        }
    }

    pub fn into_parts(self) -> (Arc<SpinLock<PagerState>>, Cursor) {
        (self.pager_state, self.cursor)
    }
}

impl<S> LendingIterator for ScanIter<S>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    type Item<'a>
        = Result<(&'a S, &'a Value), DbError>
    where
        Self: 'a;

    fn next(&mut self) -> Option<Self::Item<'_>> {
        loop {
            let (page_id, idx) = self.cursor?;

            let node = match self.pager_state.acquire().pager.read_page::<S>(page_id) {
                Ok(node) => node,
                Err(err) => {
                    self.cursor = None;
                    return Some(Err(err));
                }
            };
            self.current = Some(node);

            match step(self.current.as_ref().expect("just assigned"), idx) {
                Step::Yield(i) => {
                    self.cursor = Some((page_id, idx + 1));
                    let node = self.current.as_ref().expect("just assigned");
                    return Some(Ok((&node.keys[i], &node.values[i])));
                }
                Step::Goto(target) => self.cursor = Some((target, 0)),
                Step::Stop => self.cursor = None,
            }
        }
    }
}

pub trait Scan<S> {
    fn scan(&self) -> ScanIter<S>;
}

impl<S, LockState> Scan<S> for BTree<S, Initialized, LockState>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    fn scan(&self) -> ScanIter<S> {
        let root_id = self.pager_state().acquire().root_id;
        ScanIter::new(Arc::clone(self.pager_state()), root_id)
    }
}
