use crate::error::DbError;
use crate::pager::{PageId, Pager};
use codec::{BincodeCodec, Codec};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use spinlock::SpinLock;
use std::{marker::PhantomData, sync::Arc};
use value::{Value, ValueError};

const MIN_DEGREE: usize = 5;
const MAX_KEYS: usize = 2 * MIN_DEGREE - 1;
const MIN_KEYS: usize = MIN_DEGREE - 1;

/// Typestate marker: no root page exists yet — `get`/`put`/`delete`/`range` are not defined.
pub struct Uninitialized;
/// Typestate marker: a root page exists — the read/write API is available.
pub struct Initialized;
/// Typestate marker: mutating methods (`put`, `delete`) do not exist on this handle.
pub struct Locked;
/// Typestate marker: mutating methods are available on this handle.
pub struct Unlocked;

/// The state one [`SpinLock`] guards: the [`Pager`] and the current root page id.
///
/// These live together deliberately — every public entry point acquires the
/// lock once and reads `root_id` fresh from this shared state, rather than
/// from a per-clone cached copy that could go stale after a concurrent split.
pub struct PagerState {
    /// The pager backing this tree's storage.
    pub pager: Pager,
    /// The page id of the current root node. Changes when the root splits or shrinks.
    pub root_id: PageId,
}

/// A disk-backed B+tree, generic over its key type and two compile-time states.
///
/// `State` (`Uninitialized`/`Initialized`) and `LockState` (`Locked`/`Unlocked`)
/// are phantom typestate parameters: methods that don't make sense in a given
/// state — `put` on a `Locked` handle, any data method on an `Uninitialized`
/// one — don't exist as methods at all, rather than panicking or returning an
/// error at runtime. See the crate's typestate `impl` blocks for which methods
/// are available in which state.
///
/// Despite the name, this is a B+tree, not a classic B-tree: values live only
/// in leaves, and leaves are chained by [`Node::next`] for fast sequential
/// scans. The name stayed for historical reasons — see the workspace README.
pub struct BTree<S, State = Uninitialized, LockState = Locked> {
    pager_state: Arc<SpinLock<PagerState>>,
    state: PhantomData<State>,
    lock_state: PhantomData<LockState>,
    _marker: PhantomData<S>,
}

impl<S, State, LockState> BTree<S, State, LockState> {
    /// Borrows the shared, lock-guarded pager state underlying this tree.
    ///
    /// Exposed for sibling crates (`scan`, `kvdb`) that need to build their
    /// own cursor over the same storage; not intended for direct use by
    /// callers of `kvdb`.
    pub fn pager_state(&self) -> &Arc<SpinLock<PagerState>> {
        &self.pager_state
    }
}

/// One page's worth of the tree: either a leaf holding real entries, or an
/// internal node holding only routing keys and child pointers.
///
/// Both shapes share one struct rather than being separate types — an
/// internal node with `values` set, or a leaf with `children` set, is a
/// compile-time-legal but semantically invalid state that only this crate's
/// `invariants` test module rejects. See `Node::leaf` and `Node::internal`
/// for the two ways a value of this type is meant to be constructed.
#[derive(Debug, Serialize, Deserialize)]
pub struct Node<S> {
    /// This node's keys, always kept sorted. In a leaf, `keys[i]` pairs with
    /// `values[i]`; in an internal node, `keys[i]` separates `children[i]`
    /// from `children[i + 1]`.
    pub keys: Vec<S>,
    /// The values stored at each key. Empty for an internal node — values live only in leaves.
    pub values: Vec<Value>,
    /// Child page ids. Empty for a leaf; `keys.len() + 1` entries for an internal node.
    pub children: Vec<PageId>,
    /// The next leaf to the right in key order, or `None` for the rightmost
    /// leaf. Always `None` on an internal node. This is what lets `scan`,
    /// `range`, and `len` walk the tree as a linked list instead of an
    /// in-order traversal.
    pub next: Option<PageId>,
    /// Whether this node is a leaf (`true`) or an internal routing node (`false`).
    pub is_leaf: bool,
}

impl<S> Node<S>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    fn new_leaf(pager: &mut Pager) -> Result<PageId, DbError> {
        let node: Node<S> = Node::leaf(Vec::new(), Vec::new(), None);
        let id = pager.allocate_page();
        pager.write_page(id, &node)?;
        Ok(id)
    }

    fn leaf(keys: Vec<S>, values: Vec<Value>, next: Option<PageId>) -> Self {
        Node {
            keys,
            values,
            children: Vec::new(),
            next,
            is_leaf: true,
        }
    }

    fn internal(keys: Vec<S>, children: Vec<PageId>) -> Self {
        Node {
            keys,
            values: Vec::new(),
            children,
            next: None,
            is_leaf: false,
        }
    }

    fn is_full(&self) -> bool {
        self.keys.len() == MAX_KEYS
    }

    fn child_for(&self, key: &S) -> PageId {
        self.children[self.child_index(key)]
    }

    fn child_index(&self, key: &S) -> usize {
        let mut i = 0;
        while i < self.keys.len() && key >= &self.keys[i] {
            i += 1;
        }
        i
    }

    fn slot(&self, key: &S) -> Option<usize> {
        self.keys.iter().position(|candidate| candidate == key)
    }
}

impl<S, State, LockState> Clone for BTree<S, State, LockState>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    fn clone(&self) -> Self {
        BTree {
            pager_state: Arc::clone(&self.pager_state),
            state: PhantomData,
            lock_state: PhantomData,
            _marker: PhantomData,
        }
    }
}

impl<S> BTree<S, Uninitialized, Locked>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    /// Opens (creating if absent) a tree at `path`, using [`BincodeCodec`] as the wire format.
    ///
    /// Always allocates a fresh root leaf, whether or not `path` already
    /// contains data — the root page id is not yet persisted across restarts.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Io`] if the file cannot be opened.
    pub fn new(path: &str) -> Result<BTree<S, Initialized, Locked>, DbError> {
        Self::new_with_codec(path, Box::new(BincodeCodec))
    }

    /// Opens (creating if absent) a tree at `path`, with an explicitly chosen [`Codec`].
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Io`] if the file cannot be opened.
    pub fn new_with_codec(
        path: &str,
        codec: Box<dyn Codec>,
    ) -> Result<BTree<S, Initialized, Locked>, DbError> {
        let pager_state = Arc::new(SpinLock::new(PagerState {
            pager: Pager::open_with(path, codec)?,
            root_id: 0,
        }));

        {
            let mut guard = pager_state.acquire();
            let root_id = Node::<S>::new_leaf(&mut guard.pager)?;
            guard.root_id = root_id;
        }

        Ok(BTree {
            pager_state,
            state: PhantomData,
            lock_state: PhantomData,
            _marker: PhantomData,
        })
    }
}

impl<S> BTree<S, Initialized, Locked>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    /// Consumes a `Locked` tree and returns an `Unlocked` one, at compile time — no I/O.
    pub fn unlock(self) -> BTree<S, Initialized, Unlocked> {
        BTree {
            pager_state: self.pager_state,
            state: PhantomData,
            lock_state: PhantomData,
            _marker: PhantomData,
        }
    }
}

impl<S, LockState> BTree<S, Initialized, LockState>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    fn search_node<R>(pager: &mut Pager, id: PageId, key: &S) -> Result<R, DbError>
    where
        R: TryFrom<Value, Error = ValueError>,
    {
        let leaf: Node<S> = Self::descend_to_leaf(pager, id, key)?;
        match leaf.slot(key) {
            Some(i) => Ok(R::try_from(leaf.values[i].clone())?),
            None => Err(ValueError::NotFound.into()),
        }
    }

    fn descend_to_leaf(pager: &mut Pager, mut id: PageId, key: &S) -> Result<Node<S>, DbError> {
        loop {
            let node: Node<S> = pager.read_page(id)?;
            if node.is_leaf {
                return Ok(node);
            }
            id = node.child_for(key);
        }
    }

    fn leftmost_leaf(pager: &mut Pager, mut id: PageId) -> Result<PageId, DbError> {
        loop {
            let node: Node<S> = pager.read_page(id)?;
            if node.is_leaf {
                return Ok(id);
            }
            id = node.children[0];
        }
    }

    fn walk_leaves<F>(pager: &mut Pager, root_id: PageId, mut visit: F) -> Result<(), DbError>
    where
        F: FnMut(Node<S>),
    {
        let mut next = Some(Self::leftmost_leaf(pager, root_id)?);
        while let Some(id) = next {
            let leaf: Node<S> = pager.read_page(id)?;
            next = leaf.next;
            visit(leaf);
        }
        Ok(())
    }

    fn find_len(pager: &mut Pager, id: PageId) -> Result<usize, DbError> {
        let mut total = 0;
        Self::walk_leaves(pager, id, |leaf| total += leaf.keys.len())?;
        Ok(total)
    }

    /// Every entry, sorted by key, cloned into a fresh `Vec`.
    ///
    /// Holds the lock for the whole traversal, so unlike the `scan` crate's
    /// lending iterator (which re-acquires the lock per step), this always
    /// sees a consistent tree even under concurrent writers.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if a page cannot be read or decoded.
    pub fn range(&mut self) -> Result<Vec<(S, Value)>, DbError> {
        let mut result = Vec::new();
        let mut guard = self.pager_state.acquire();
        let root_id = guard.root_id;
        Self::walk_leaves(&mut guard.pager, root_id, |leaf| {
            result.extend(std::iter::zip(leaf.keys, leaf.values));
        })?;
        Ok(result)
    }

    /// Looks up `key` and converts its value to `R`.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Value(ValueError::NotFound)` if the key is absent,
    /// `DbError::Value(ValueError::TypeMismatch)` if the stored value is not
    /// exactly `R`, or another [`DbError`] variant on an I/O or decode failure.
    pub fn get<R>(&mut self, key: &S) -> Result<R, DbError>
    where
        R: TryFrom<Value, Error = ValueError>,
    {
        let mut guard = self.pager_state.acquire();
        let root_id = guard.root_id;
        Self::search_node(&mut guard.pager, root_id, key)
    }

    /// The number of entries in the tree. Walks every leaf — not O(1).
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if a page cannot be read or decoded.
    pub fn len(&mut self) -> Result<usize, DbError> {
        let mut guard = self.pager_state.acquire();
        let root_id = guard.root_id;
        Self::find_len(&mut guard.pager, root_id)
    }

    /// `len() == 0`, with the same full-walk cost as [`BTree::len`].
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if a page cannot be read or decoded.
    pub fn is_empty(&mut self) -> Result<bool, DbError> {
        Ok(self.len()? == 0)
    }
}

impl<S> BTree<S, Initialized, Unlocked>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    fn insert(&mut self, key: S, value: Value) -> Result<(), DbError> {
        let mut guard = self.pager_state.acquire();
        Self::insert_locked(&mut guard, key, value)
    }

    fn insert_locked(state: &mut PagerState, key: S, value: Value) -> Result<(), DbError> {
        let root_id = state.root_id;
        let root_is_full: bool = {
            let node: Node<S> = state.pager.read_page(root_id)?;
            node.is_full()
        };

        if root_is_full {
            let new_root_node: Node<S> = Node::internal(Vec::new(), vec![root_id]);
            let new_root_id = state.pager.allocate_page();
            state.pager.write_page(new_root_id, &new_root_node)?;

            Self::split_child(&mut state.pager, new_root_id, 0)?;
            state.root_id = new_root_id;
        }

        let root_id = state.root_id;
        Self::insert_non_full(&mut state.pager, root_id, key, value)
    }

    fn split_child(pager: &mut Pager, parent_id: PageId, i: usize) -> Result<(), DbError> {
        let mid = MIN_DEGREE - 1;
        let mut parent: Node<S> = pager.read_page(parent_id)?;
        let child_id = parent.children[i];
        let mut child: Node<S> = pager.read_page(child_id)?;
        let right_id = pager.allocate_page();

        let separator = if child.is_leaf {
            let right_keys = child.keys.split_off(mid);
            let right_values = child.values.split_off(mid);
            let separator = right_keys[0].clone();
            let right = Node::leaf(right_keys, right_values, child.next);
            child.next = Some(right_id);
            pager.write_page(right_id, &right)?;
            separator
        } else {
            let right_keys = child.keys.split_off(mid + 1);
            let separator = child.keys.pop().expect("a full node has a middle key");
            let right_children = child.children.split_off(mid + 1);
            let right = Node::internal(right_keys, right_children);
            pager.write_page(right_id, &right)?;
            separator
        };

        pager.write_page(child_id, &child)?;

        parent.keys.insert(i, separator);
        parent.children.insert(i + 1, right_id);
        pager.write_page(parent_id, &parent)
    }

    fn insert_non_full(pager: &mut Pager, id: PageId, key: S, value: Value) -> Result<(), DbError> {
        let mut node: Node<S> = pager.read_page(id)?;

        if node.is_leaf {
            if let Some(i) = node.slot(&key) {
                node.values[i].accumulate(value);
                return pager.write_page(id, &node);
            }

            let mut i = node.keys.len();
            while i > 0 && key < node.keys[i - 1] {
                i -= 1;
            }
            node.keys.insert(i, key);
            node.values.insert(i, value);
            return pager.write_page(id, &node);
        }

        let mut i = node.child_index(&key);
        let child_id = node.children[i];
        let child_full = {
            let child: Node<S> = pager.read_page(child_id)?;
            child.is_full()
        };

        if child_full {
            Self::split_child(pager, id, i)?;
            node = pager.read_page(id)?;
            if key >= node.keys[i] {
                i += 1;
            }
        }

        let child_id = node.children[i];
        Self::insert_non_full(pager, child_id, key, value)
    }

    fn delete_node(
        pager: &mut Pager,
        id: PageId,
        key: S,
    ) -> Result<(bool, Option<Value>), DbError> {
        let mut node: Node<S> = pager.read_page(id)?;

        if node.is_leaf {
            return match node.slot(&key) {
                Some(i) => {
                    node.keys.remove(i);
                    let value = node.values.remove(i);
                    pager.write_page(id, &node)?;
                    Ok((true, Some(value)))
                }
                None => Ok((false, None)),
            };
        }

        let child_id = Self::fill_child(pager, id, node.child_index(&key))?;
        Self::delete_node(pager, child_id, key)
    }

    fn fill_child(pager: &mut Pager, parent_id: PageId, idx: usize) -> Result<PageId, DbError> {
        let parent: Node<S> = pager.read_page(parent_id)?;
        let child_id = parent.children[idx];
        let child_len = pager.read_page::<S>(child_id)?.keys.len();
        if child_len > MIN_KEYS {
            return Ok(child_id);
        }

        let n = parent.children.len();

        let left_has_spare = if idx > 0 {
            let left_id = parent.children[idx - 1];
            pager.read_page::<S>(left_id)?.keys.len() > MIN_KEYS
        } else {
            false
        };
        if left_has_spare {
            Self::borrow_from_prev(pager, parent_id, idx)?;
            return Ok(child_id);
        }

        let right_has_spare = if idx < n - 1 {
            let right_id = parent.children[idx + 1];
            pager.read_page::<S>(right_id)?.keys.len() > MIN_KEYS
        } else {
            false
        };
        if right_has_spare {
            Self::borrow_from_next(pager, parent_id, idx)?;
            return Ok(child_id);
        }

        if idx < n - 1 {
            Self::merge_children(pager, parent_id, idx)?;
            Ok(child_id)
        } else {
            let left_id = parent.children[idx - 1];
            Self::merge_children(pager, parent_id, idx - 1)?;
            Ok(left_id)
        }
    }

    fn borrow_from_prev(pager: &mut Pager, parent_id: PageId, idx: usize) -> Result<(), DbError> {
        let mut parent: Node<S> = pager.read_page(parent_id)?;
        let child_id = parent.children[idx];
        let left_id = parent.children[idx - 1];
        let mut child: Node<S> = pager.read_page(child_id)?;
        let mut left: Node<S> = pager.read_page(left_id)?;

        if child.is_leaf {
            let key = left.keys.pop().expect("left sibling has spare keys");
            let value = left.values.pop().expect("left sibling has spare values");
            child.keys.insert(0, key);
            child.values.insert(0, value);
            parent.keys[idx - 1] = child.keys[0].clone();
        } else {
            child.keys.insert(0, parent.keys[idx - 1].clone());
            let moved_child = left
                .children
                .pop()
                .expect("internal sibling always has children");
            child.children.insert(0, moved_child);
            parent.keys[idx - 1] = left.keys.pop().expect("left sibling has spare keys");
        }

        pager.write_page(child_id, &child)?;
        pager.write_page(left_id, &left)?;
        pager.write_page(parent_id, &parent)
    }

    fn borrow_from_next(pager: &mut Pager, parent_id: PageId, idx: usize) -> Result<(), DbError> {
        let mut parent: Node<S> = pager.read_page(parent_id)?;
        let child_id = parent.children[idx];
        let right_id = parent.children[idx + 1];
        let mut child: Node<S> = pager.read_page(child_id)?;
        let mut right: Node<S> = pager.read_page(right_id)?;

        if child.is_leaf {
            child.keys.push(right.keys.remove(0));
            child.values.push(right.values.remove(0));
            parent.keys[idx] = right.keys[0].clone();
        } else {
            child.keys.push(parent.keys[idx].clone());
            let moved_child = right.children.remove(0);
            child.children.push(moved_child);
            parent.keys[idx] = right.keys.remove(0);
        }

        pager.write_page(child_id, &child)?;
        pager.write_page(right_id, &right)?;
        pager.write_page(parent_id, &parent)
    }

    fn merge_children(pager: &mut Pager, parent_id: PageId, idx: usize) -> Result<(), DbError> {
        let mut parent: Node<S> = pager.read_page(parent_id)?;
        let left_id = parent.children[idx];
        let right_id = parent.children[idx + 1];
        let mut left: Node<S> = pager.read_page(left_id)?;
        let right: Node<S> = pager.read_page(right_id)?;

        let separator = parent.keys.remove(idx);
        parent.children.remove(idx + 1);

        if left.is_leaf {
            left.keys.extend(right.keys);
            left.values.extend(right.values);
            left.next = right.next;
        } else {
            left.keys.push(separator);
            left.keys.extend(right.keys);
            left.children.extend(right.children);
        }

        pager.write_page(left_id, &left)?;
        pager.write_page(parent_id, &parent)
    }

    fn update_node(pager: &mut Pager, id: PageId, key: &S, value: &Value) -> Result<bool, DbError> {
        let mut id = id;
        loop {
            let mut node: Node<S> = pager.read_page(id)?;
            if node.is_leaf {
                return match node.slot(key) {
                    Some(i) => {
                        node.values[i] = value.clone();
                        pager.write_page(id, &node)?;
                        Ok(true)
                    }
                    None => Ok(false),
                };
            }
            id = node.child_for(key);
        }
    }

    /// Consumes an `Unlocked` tree and returns a `Locked` one, at compile time — no I/O.
    pub fn lock(self) -> BTree<S, Initialized, Locked> {
        BTree {
            pager_state: self.pager_state,
            state: PhantomData,
            lock_state: PhantomData,
            _marker: PhantomData,
        }
    }

    /// Inserts `value` under `key`.
    ///
    /// If `key` already holds a value, the new value is *accumulated* rather
    /// than overwriting it: repeated `put`s of the same key fold every value
    /// into one [`Value::Multi`], readable back as `Vec<Value>`. Use
    /// [`BTree::update`] for replace-in-place (upsert) semantics.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if a page cannot be read, written, or would
    /// overflow the fixed page size.
    pub fn put(&mut self, key: S, value: impl Into<Value>) -> Result<(), DbError> {
        self.insert(key, value.into())
    }

    /// Replaces the value at `key` if it exists, otherwise inserts it — upsert semantics.
    ///
    /// Unlike [`BTree::put`], a repeat `update` on an existing key discards
    /// whatever was there (including an accumulator built by `put`) rather
    /// than folding into it. The lookup and the fallback insert happen under
    /// one lock acquisition, so concurrent `update`s racing on the same
    /// absent key cannot both insert.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if a page cannot be read, written, or would
    /// overflow the fixed page size.
    pub fn update(&mut self, key: S, value: impl Into<Value>) -> Result<(), DbError> {
        let value = value.into();
        let mut guard = self.pager_state.acquire();
        let root_id = guard.root_id;
        if Self::update_node(&mut guard.pager, root_id, &key, &value)? {
            return Ok(());
        }
        Self::insert_locked(&mut guard, key, value)
    }

    /// Removes `key` if present, returning `(true, Some(previous_value))`, or
    /// `(false, None)` if it was absent.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if a page cannot be read or written.
    pub fn delete(&mut self, key: S) -> Result<(bool, Option<Value>), DbError> {
        let mut guard = self.pager_state.acquire();
        let root_id = guard.root_id;
        let result = Self::delete_node(&mut guard.pager, root_id, key)?;

        let root: Node<S> = guard.pager.read_page(root_id)?;
        if !root.is_leaf && root.keys.is_empty() {
            guard.root_id = root.children[0];
        }

        Ok(result)
    }
}
