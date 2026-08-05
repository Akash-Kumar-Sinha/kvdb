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

#[doc(hidden)]
pub enum Step {
    Yield(usize),
    Descend(PageId),
    Pop,
}

#[doc(hidden)]
pub fn step<S>(node: &Node<S>, idx: usize) -> Step {
    if node.is_leaf {
        return if idx < node.keys.len() {
            Step::Yield(idx)
        } else {
            Step::Pop
        };
    }

    if idx.is_multiple_of(2) {
        let child_index = idx / 2;
        if child_index < node.children.len() {
            Step::Descend(node.children[child_index])
        } else {
            Step::Pop
        }
    } else {
        let key_index = (idx - 1) / 2;
        if key_index < node.keys.len() {
            Step::Yield(key_index)
        } else {
            Step::Pop
        }
    }
}

pub struct ScanIter<S> {
    pager_state: Arc<SpinLock<PagerState>>,
    stack: Vec<(PageId, usize)>,
    current: Option<Node<S>>,
}

impl<S> ScanIter<S> {
    #[doc(hidden)]
    pub fn new(pager_state: Arc<SpinLock<PagerState>>, root_id: PageId) -> Self {
        ScanIter {
            pager_state,
            stack: vec![(root_id, 0)],
            current: None,
        }
    }

    #[doc(hidden)]
    pub fn into_parts(self) -> (Arc<SpinLock<PagerState>>, Vec<(PageId, usize)>) {
        (self.pager_state, self.stack)
    }

    fn advance_cursor(&mut self) {
        self.stack
            .last_mut()
            .expect("stack is non-empty while stepping")
            .1 += 1;
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
            let (page_id, idx) = *self.stack.last()?;

            let node = match self.pager_state.acquire().pager.read_page::<S>(page_id) {
                Ok(node) => node,
                Err(err) => {
                    self.stack.clear();
                    return Some(Err(err));
                }
            };
            self.current = Some(node);
            let node = self.current.as_ref().expect("just assigned");

            match step(node, idx) {
                Step::Yield(i) => {
                    self.advance_cursor();
                    let node = self.current.as_ref().expect("just assigned");
                    return Some(Ok((&node.keys[i], &node.values[i])));
                }
                Step::Descend(child_id) => {
                    self.advance_cursor();
                    self.stack.push((child_id, 0));
                }
                Step::Pop => {
                    self.stack.pop();
                }
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
