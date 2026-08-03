use crate::pager::{PageId, Pager};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::marker::PhantomData;

const MIN_DEGREE: usize = 5;
const MAX_KEYS: usize = 2 * MIN_DEGREE - 1;
const MIN_KEYS: usize = MIN_DEGREE - 1;

pub struct Uninitialized;
pub struct Initialized;
pub struct Locked;
pub struct Unlocked;

pub struct BTree<S, T, State = Uninitialized, LockState = Locked> {
    pager: Pager,
    root_id: PageId,
    state: PhantomData<State>,
    lock_state: PhantomData<LockState>,
    _marker: PhantomData<(S, T)>,
}

#[derive(Serialize, Deserialize)]
pub struct Node<S, T> {
    pub keys: Vec<S>,
    pub values: Vec<T>,
    pub children: Vec<PageId>,
    pub is_leaf: bool,
}

impl<S, T> Node<S, T>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
    T: Clone + Serialize + DeserializeOwned,
{
    fn new_leaf(pager: &mut Pager) -> PageId {
        let node: Node<S, T> = Node {
            keys: Vec::new(),
            values: Vec::new(),
            children: Vec::new(),
            is_leaf: true,
        };
        let id = pager.allocate_page();
        pager.write_page(id, &node).expect("write failed");
        id
    }

    fn is_full(&self) -> bool {
        self.keys.len() == MAX_KEYS
    }
}

impl<S, T> BTree<S, T, Uninitialized, Locked>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
    T: Clone + Serialize + DeserializeOwned,
{
    pub fn new(path: &str) -> BTree<S, T, Initialized, Locked> {
        let mut pager = Pager::open(path).expect("open failed");
        let root_id = Node::<S, T>::new_leaf(&mut pager);
        BTree {
            pager,
            root_id,
            state: PhantomData,
            lock_state: PhantomData,
            _marker: PhantomData,
        }
    }
}

impl<S, T> BTree<S, T, Initialized, Locked>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
    T: Clone + Serialize + DeserializeOwned,
{
    pub fn unlock(self) -> BTree<S, T, Initialized, Unlocked> {
        BTree {
            pager: self.pager,
            root_id: self.root_id,
            state: PhantomData,
            lock_state: PhantomData,
            _marker: PhantomData,
        }
    }
}

impl<S, T, LockState> BTree<S, T, Initialized, LockState>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
    T: Clone + Serialize + DeserializeOwned,
{
    fn search_node(pager: &mut Pager, id: PageId, key: &S) -> Option<T> {
        let node: Node<S, T> = pager.read_page(id).expect("read failed");

        let mut i = 0;
        while i < node.keys.len() && key > &node.keys[i] {
            i += 1;
        }
        if i < node.keys.len() && key == &node.keys[i] {
            return Some(node.values[i].clone());
        }
        if node.is_leaf {
            return None;
        }
        let child_id = node.children[i];
        Self::search_node(pager, child_id, key)
    }

    fn range_node(pager: &mut Pager, id: PageId, result: &mut Vec<(S, T)>) {
        let node: Node<S, T> = pager.read_page(id).expect("read failed");

        for i in 0..node.keys.len() {
            if !node.is_leaf {
                Self::range_node(pager, node.children[i], result);
            }
            result.push((node.keys[i].clone(), node.values[i].clone()));
        }
        if !node.is_leaf {
            let last_child = node.children[node.keys.len()];
            Self::range_node(pager, last_child, result);
        }
    }

    fn find_len(pager: &mut Pager, id: PageId) -> usize {
        let node: Node<S, T> = pager.read_page(id).expect("read failed");
        let mut total = node.keys.len();
        for &child_id in &node.children {
            total += Self::find_len(pager, child_id);
        }
        total
    }

    pub fn range(&mut self) -> Vec<(S, T)> {
        let mut result = Vec::new();
        let root_id = self.root_id;
        Self::range_node(&mut self.pager, root_id, &mut result);
        result
    }

    pub fn get(&mut self, key: &S) -> Option<T> {
        let root_id = self.root_id;
        Self::search_node(&mut self.pager, root_id, key)
    }

    pub fn len(&mut self) -> usize {
        let root_id = self.root_id;
        Self::find_len(&mut self.pager, root_id)
    }
}

impl<S, T> BTree<S, T, Initialized, Unlocked>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
    T: Clone + Serialize + DeserializeOwned,
{
    fn insert(&mut self, key: S, value: T) {
        let root_is_full: bool = {
            let node: Node<S, T> = self.pager.read_page(self.root_id).expect("read failed");
            node.is_full()
        };

        if root_is_full {
            let new_root_node: Node<S, T> = Node {
                keys: Vec::new(),
                values: Vec::new(),
                children: vec![self.root_id],
                is_leaf: false,
            };
            let new_root_id = self.pager.allocate_page();
            self.pager
                .write_page(new_root_id, &new_root_node)
                .expect("write failed");

            Self::split_child(&mut self.pager, new_root_id, 0);
            self.root_id = new_root_id;
        }

        let root_id = self.root_id;
        Self::insert_non_full(&mut self.pager, root_id, key, value);
    }

    fn split_child(pager: &mut Pager, parent_id: PageId, i: usize) {
        let mid = MIN_DEGREE - 1;
        let mut parent: Node<S, T> = pager.read_page(parent_id).expect("read failed");
        let child_id = parent.children[i];
        let mut child: Node<S, T> = pager.read_page(child_id).expect("read failed");

        let mid_key = child.keys[mid].clone();
        let mid_val = child.values[mid].clone();
        let right_keys = child.keys.split_off(mid + 1);
        let right_vals = child.values.split_off(mid + 1);
        child.keys.pop();
        child.values.pop();
        let right_children = if !child.children.is_empty() {
            child.children.split_off(mid + 1)
        } else {
            Vec::new()
        };
        let child_is_leaf = child.is_leaf;

        let right_node = Node {
            keys: right_keys,
            values: right_vals,
            children: right_children,
            is_leaf: child_is_leaf,
        };

        let right_id = pager.allocate_page();
        pager
            .write_page(right_id, &right_node)
            .expect("write failed");
        pager.write_page(child_id, &child).expect("write failed");

        parent.keys.insert(i, mid_key);
        parent.values.insert(i, mid_val);
        parent.children.insert(i + 1, right_id);
        pager.write_page(parent_id, &parent).expect("write failed");
    }

    fn insert_non_full(pager: &mut Pager, id: PageId, key: S, value: T) {
        let mut node: Node<S, T> = pager.read_page(id).expect("read failed");

        if node.is_leaf {
            let mut i = node.keys.len();
            while i > 0 && key < node.keys[i - 1] {
                i -= 1;
            }
            node.keys.insert(i, key);
            node.values.insert(i, value);
            pager.write_page(id, &node).expect("write failed");
            return;
        }

        let mut i = node.keys.len();
        while i > 0 && key < node.keys[i - 1] {
            i -= 1;
        }

        let child_id = node.children[i];
        let child_full = {
            let child: Node<S, T> = pager.read_page(child_id).expect("read failed");
            child.is_full()
        };

        if child_full {
            Self::split_child(pager, id, i);
            node = pager.read_page(id).expect("read failed");
            if key > node.keys[i] {
                i += 1;
            }
        }

        let child_id = node.children[i];
        Self::insert_non_full(pager, child_id, key, value);
    }

    fn delete_node(pager: &mut Pager, id: PageId, key: S) -> (bool, Option<T>) {
        let mut node: Node<S, T> = pager.read_page(id).expect("read failed");

        let mut i = 0;
        while i < node.keys.len() && key > node.keys[i] {
            i += 1;
        }

        if i < node.keys.len() && key == node.keys[i] {
            if node.is_leaf {
                node.keys.remove(i);
                let val = node.values.remove(i);
                pager.write_page(id, &node).expect("write failed");
                return (true, Some(val));
            }

            let left_id = node.children[i];
            let right_id = node.children[i + 1];
            let left_len = pager
                .read_page::<S, T>(left_id)
                .expect("read failed")
                .keys
                .len();
            let right_len = pager
                .read_page::<S, T>(right_id)
                .expect("read failed")
                .keys
                .len();
            let original_val = node.values[i].clone();

            if left_len > MIN_KEYS {
                let (pred_key, pred_val) = Self::get_predecessor(pager, left_id);
                node.keys[i] = pred_key.clone();
                node.values[i] = pred_val;
                pager.write_page(id, &node).expect("write failed");
                Self::delete_node(pager, left_id, pred_key);
                return (true, Some(original_val));
            } else if right_len > MIN_KEYS {
                let (succ_key, succ_val) = Self::get_successor(pager, right_id);
                node.keys[i] = succ_key.clone();
                node.values[i] = succ_val;
                pager.write_page(id, &node).expect("write failed");
                Self::delete_node(pager, right_id, succ_key);
                return (true, Some(original_val));
            } else {
                Self::merge_children(pager, id, i);
                Self::delete_node(pager, left_id, key);
                return (true, Some(original_val));
            }
        }

        if node.is_leaf {
            return (false, None);
        }

        let child_id = Self::fill_child(pager, id, i);
        Self::delete_node(pager, child_id, key)
    }

    fn get_predecessor(pager: &mut Pager, mut id: PageId) -> (S, T) {
        loop {
            let node: Node<S, T> = pager.read_page(id).expect("read failed");
            if node.is_leaf {
                let last = node.keys.len() - 1;
                return (node.keys[last].clone(), node.values[last].clone());
            }
            id = *node.children.last().unwrap();
        }
    }

    fn get_successor(pager: &mut Pager, mut id: PageId) -> (S, T) {
        loop {
            let node: Node<S, T> = pager.read_page(id).expect("read failed");
            if node.is_leaf {
                return (node.keys[0].clone(), node.values[0].clone());
            }
            id = node.children[0];
        }
    }

    fn fill_child(pager: &mut Pager, parent_id: PageId, idx: usize) -> PageId {
        let parent: Node<S, T> = pager.read_page(parent_id).expect("read failed");
        let child_id = parent.children[idx];
        let child_len = pager
            .read_page::<S, T>(child_id)
            .expect("read failed")
            .keys
            .len();
        if child_len > MIN_KEYS {
            return child_id;
        }

        let n = parent.children.len();

        let left_has_spare = idx > 0 && {
            let left_id = parent.children[idx - 1];
            pager
                .read_page::<S, T>(left_id)
                .expect("read failed")
                .keys
                .len()
                > MIN_KEYS
        };
        if left_has_spare {
            Self::borrow_from_prev(pager, parent_id, idx);
            return child_id;
        }

        let right_has_spare = idx < n - 1 && {
            let right_id = parent.children[idx + 1];
            pager
                .read_page::<S, T>(right_id)
                .expect("read failed")
                .keys
                .len()
                > MIN_KEYS
        };
        if right_has_spare {
            Self::borrow_from_next(pager, parent_id, idx);
            return child_id;
        }

        if idx < n - 1 {
            Self::merge_children(pager, parent_id, idx);
            child_id
        } else {
            let left_id = parent.children[idx - 1];
            Self::merge_children(pager, parent_id, idx - 1);
            left_id
        }
    }

    fn borrow_from_prev(pager: &mut Pager, parent_id: PageId, idx: usize) {
        let mut parent: Node<S, T> = pager.read_page(parent_id).expect("read failed");
        let child_id = parent.children[idx];
        let left_id = parent.children[idx - 1];
        let mut child: Node<S, T> = pager.read_page(child_id).expect("read failed");
        let mut left: Node<S, T> = pager.read_page(left_id).expect("read failed");

        child.keys.insert(0, parent.keys[idx - 1].clone());
        child.values.insert(0, parent.values[idx - 1].clone());
        if !left.children.is_empty() {
            let moved_child = left.children.pop().unwrap();
            child.children.insert(0, moved_child);
        }

        let left_last_key = left.keys.pop().unwrap();
        let left_last_val = left.values.pop().unwrap();
        parent.keys[idx - 1] = left_last_key;
        parent.values[idx - 1] = left_last_val;

        pager.write_page(child_id, &child).expect("write failed");
        pager.write_page(left_id, &left).expect("write failed");
        pager.write_page(parent_id, &parent).expect("write failed");
    }

    fn borrow_from_next(pager: &mut Pager, parent_id: PageId, idx: usize) {
        let mut parent: Node<S, T> = pager.read_page(parent_id).expect("read failed");
        let child_id = parent.children[idx];
        let right_id = parent.children[idx + 1];
        let mut child: Node<S, T> = pager.read_page(child_id).expect("read failed");
        let mut right: Node<S, T> = pager.read_page(right_id).expect("read failed");

        child.keys.push(parent.keys[idx].clone());
        child.values.push(parent.values[idx].clone());
        if !right.children.is_empty() {
            let moved_child = right.children.remove(0);
            child.children.push(moved_child);
        }

        let right_first_key = right.keys.remove(0);
        let right_first_val = right.values.remove(0);
        parent.keys[idx] = right_first_key;
        parent.values[idx] = right_first_val;

        pager.write_page(child_id, &child).expect("write failed");
        pager.write_page(right_id, &right).expect("write failed");
        pager.write_page(parent_id, &parent).expect("write failed");
    }

    fn merge_children(pager: &mut Pager, parent_id: PageId, idx: usize) {
        let mut parent: Node<S, T> = pager.read_page(parent_id).expect("read failed");
        let left_id = parent.children[idx];
        let right_id = parent.children[idx + 1];
        let mut left: Node<S, T> = pager.read_page(left_id).expect("read failed");
        let right: Node<S, T> = pager.read_page(right_id).expect("read failed");

        let mid_key = parent.keys.remove(idx);
        let mid_val = parent.values.remove(idx);
        parent.children.remove(idx + 1);

        left.keys.push(mid_key);
        left.values.push(mid_val);
        left.keys.extend(right.keys);
        left.values.extend(right.values);
        left.children.extend(right.children);

        pager.write_page(left_id, &left).expect("write failed");
        pager.write_page(parent_id, &parent).expect("write failed");
    }

    pub fn lock(self) -> BTree<S, T, Initialized, Locked> {
        BTree {
            pager: self.pager,
            root_id: self.root_id,
            state: PhantomData,
            lock_state: PhantomData,
            _marker: PhantomData,
        }
    }

    pub fn put(&mut self, key: S, value: T) {
        self.insert(key, value);
    }

    pub fn delete(&mut self, key: S) -> (bool, Option<T>) {
        let result = Self::delete_node(&mut self.pager, self.root_id, key);

        let root: Node<S, T> = self.pager.read_page(self.root_id).expect("read failed");
        if !root.is_leaf && root.keys.is_empty() {
            self.root_id = root.children[0];
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pager::Pager;

    #[cfg(test)]
    fn fresh_pager(path: &str) -> Pager {
        std::fs::remove_file(path).ok();
        Pager::open(path).expect("open failed")
    }

    #[test]
    fn test_insertion_structure() {
        let path = "/tmp/test_btree.db";

        let tree = BTree::<i32, i32, Uninitialized, Locked>::new(path);
        let mut tree = tree.unlock();

        for i in 1..=12 {
            tree.put(i, i * 10);
        }

        for i in 1..=12 {
            assert_eq!(tree.get(&i), Some(i * 10), "get({i}) mismatch");
        }
        assert_eq!(tree.get(&999), None, "missing key should return None");

        let range = tree.range();
        let expected: Vec<(i32, i32)> = (1..=12).map(|i| (i, i * 10)).collect();
        assert_eq!(
            range, expected,
            "range() must return sorted (key, value) pairs"
        );

        assert_eq!(tree.len(), 12);

        let (found, value) = tree.delete(12);
        assert!(found, "delete(12) should report found = true");
        assert_eq!(value, Some(120));
        assert_eq!(tree.get(&12), None, "12 should be gone after delete");
        assert_eq!(tree.len(), 11);

        let (found, value) = tree.delete(999);
        assert!(!found, "deleting a missing key should report found = false");
        assert_eq!(value, None);
        assert_eq!(
            tree.len(),
            11,
            "len should be unchanged after a failed delete"
        );

        let (found, value) = tree.delete(5);
        assert!(found);
        assert_eq!(value, Some(50));
        assert_eq!(tree.get(&5), None);
        assert_eq!(tree.len(), 10);

        let mut tree = tree.lock();
        assert_eq!(tree.get(&1), Some(10));

        std::fs::remove_file(path).ok();
    }
}
