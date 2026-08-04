use std::sync::Arc;

use btree::{BTree, Initialized, Node, PageId, PagerState, Value};
use serde::{Serialize, de::DeserializeOwned};
use spinlock::SpinLock;

pub trait LendingIterator {
    type Item<'a>
    where
        Self: 'a;
    fn next<'a>(&'a mut self) -> Option<Self::Item<'a>>;
}
pub struct ScanIter<S> {
    pub pager_state: Arc<SpinLock<PagerState>>,
    pub stack: Vec<(PageId, usize)>,
    pub current: Option<Node<S>>,
}

impl<S> LendingIterator for ScanIter<S>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    type Item<'a>
        = (&'a S, &'a Value)
    where
        Self: 'a;

    fn next<'a>(&'a mut self) -> Option<Self::Item<'a>> {
        loop {
            let (page_id, idx) = *self.stack.last()?;

            let node = self
                .pager_state
                .acquire()
                .pager
                .read_page::<S>(page_id)
                .expect("read failed");
            self.current = Some(node);
            let node = self.current.as_ref().unwrap();

            if node.is_leaf {
                if idx < node.keys.len() {
                    self.stack.last_mut().unwrap().1 += 1;
                    let node = self.current.as_ref().unwrap();
                    return Some((&node.keys[idx], &node.values[idx]));
                }
                self.stack.pop();
                continue;
            }

            if idx % 2 == 0 {
                let child_index = idx / 2;
                if child_index < node.children.len() {
                    let child_id = node.children[child_index];
                    self.stack.last_mut().unwrap().1 += 1;
                    self.stack.push((child_id, 0));
                    continue;
                }
                self.stack.pop();
                continue;
            } else {
                let key_index = (idx - 1) / 2;
                if key_index < node.keys.len() {
                    self.stack.last_mut().unwrap().1 += 1;
                    let node = self.current.as_ref().unwrap();
                    return Some((&node.keys[key_index], &node.values[key_index]));
                }
                self.stack.pop();
                continue;
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
        let guard = self.pager_state.acquire();
        ScanIter {
            pager_state: Arc::clone(&self.pager_state),
            stack: vec![(guard.root_id, 0)],
            current: None,
        }
    }
}
