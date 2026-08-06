use std::sync::Arc;

use btree::{BTree, DbError, Initialized, Node, PageId, PagerState, Value};
use serde::{Serialize, de::DeserializeOwned};
use spinlock::SpinLock;

/// An iterator whose items can borrow from the iterator itself, expressed via
/// a Generic Associated Type.
///
/// `std::iter::Iterator::Item` cannot depend on the lifetime of the `&mut
/// self` passed to `next`, so it cannot describe an item borrowed from the
/// iterator's own internal state. `Item<'a>` here can, which is what lets
/// [`ScanIter`] yield `(&'a S, &'a Value)` borrowed straight out of whichever
/// page it currently has loaded, with no clone.
///
/// The tradeoff is dispatch: a GAT is not dyn-compatible, so this trait
/// cannot be used as `dyn LendingIterator`, and `for` loops / `.map()` /
/// `.collect()` don't work on it — drive it by hand with
/// `while let Some(item) = iter.next() { ... }`.
pub trait LendingIterator {
    /// The item yielded by one step, allowed to borrow from `self` for `'a`.
    type Item<'a>
    where
        Self: 'a;

    /// Advances the iterator, returning the next item or `None` when exhausted.
    fn next(&mut self) -> Option<Self::Item<'_>>;
}

/// A scan's position: the page currently being read, and the index within it.
/// `None` once the scan is exhausted or has failed.
pub type Cursor = Option<(PageId, usize)>;

/// What [`step`] found at a given cursor position.
pub enum Step {
    /// Yield the entry at this index in the current leaf.
    Yield(usize),
    /// Move the cursor to this page and continue.
    Goto(PageId),
    /// The walk is over — no more pages to visit.
    Stop,
}

/// One step of the shared B+tree leaf-chain walk: given a node and a cursor
/// index into it, decide whether to yield an entry, move to another page, or
/// stop.
///
/// This is the single function both [`ScanIter`] (sync) and `async_kvdb`'s
/// `AsyncScanIter` build on, so the two walkers cannot drift out of agreement
/// about traversal order. Because this engine is a B+tree, the walk is not a
/// tree traversal at all: descend once to the leftmost leaf, yield its
/// entries in order, then follow [`Node::next`] to the next leaf.
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

/// A zero-copy, in-order cursor over a [`BTree`]'s entries.
///
/// Implements [`LendingIterator`], not `std::iter::Iterator` — see that
/// trait's docs for why, and drive this with `while let Some(item) =
/// iter.next() { ... }`. Each yielded item borrows from whichever page this
/// iterator currently has loaded, valid until the next call to `next()`.
///
/// Re-acquires the underlying lock on every `next()` call rather than holding
/// it for the whole scan, so this is not snapshot-isolated: a concurrent
/// writer can split a node between two steps and the walk may then skip or
/// repeat an entry. `BTree::range` holds the lock for the entire traversal
/// and is the consistent alternative.
pub struct ScanIter<S> {
    pager_state: Arc<SpinLock<PagerState>>,
    cursor: Cursor,
    current: Option<Node<S>>,
}

impl<S> ScanIter<S> {
    /// Starts a new scan positioned at the tree's root.
    pub fn new(pager_state: Arc<SpinLock<PagerState>>, root_id: PageId) -> Self {
        ScanIter {
            pager_state,
            cursor: Some((root_id, 0)),
            current: None,
        }
    }

    /// Decomposes this iterator into its shared lock and current [`Cursor`].
    ///
    /// Exists so `async_kvdb`'s `AsyncScanIter` can resume a scan started
    /// synchronously — the cursor is `Send`, so it can move into a job
    /// closure dispatched to the async thread pool, which the borrowed
    /// `ScanIter` itself cannot.
    pub fn into_parts(self) -> (Arc<SpinLock<PagerState>>, Cursor) {
        (self.pager_state, self.cursor)
    }
}

impl<S> LendingIterator for ScanIter<S>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    /// `Err` if the page at the cursor could not be read or decoded. After an
    /// error the cursor is cleared, so the next call returns `None` instead
    /// of retrying the page that just failed.
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

/// Adds a zero-copy [`scan`](Scan::scan) method to a tree.
pub trait Scan<S> {
    /// Starts a zero-copy, in-order cursor over every entry, borrowing rather
    /// than cloning. See [`ScanIter`] for its consistency and dispatch tradeoffs.
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
