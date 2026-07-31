use std::{cell::RefCell, rc::Rc};

type Link<S, T> = Rc<RefCell<Node<S, T>>>;

const MIN_DEGREE: usize = 4;
const MAX_KEYS: usize = 2 * MIN_DEGREE - 1;

pub struct Node<S, T> {
    keys: Vec<S>,
    values: Vec<T>,
    children: Vec<Link<S, T>>,
    is_leaf: bool,
}

impl<S, T> Node<S, T>
where
    S: Ord + Clone,
    T: Clone,
{
    fn new_leaf() -> Link<S, T> {
        Rc::new(RefCell::new(Node {
            keys: Vec::new(),
            values: Vec::new(),
            children: Vec::new(),
            is_leaf: true,
        }))
    }

    fn is_full(&self) -> bool {
        self.keys.len() == MAX_KEYS
    }
}

struct Uninitialized;
struct Initialized;
struct Locked;
struct Unlocked;

pub struct BTree<S, T, State = Uninitialized, LockState = Locked> {
    root: Link<S, T>,
    state: std::marker::PhantomData<State>,
    lock_state: std::marker::PhantomData<LockState>,
}

impl<S, T> BTree<S, T, Uninitialized, Locked>
where
    S: Ord + Clone,
    T: Clone,
{
    pub fn new() -> BTree<S, T, Initialized, Locked> {
        BTree {
            root: Node::new_leaf(),
            state: std::marker::PhantomData,
            lock_state: std::marker::PhantomData,
        }
    }
}

impl<S, T> BTree<S, T, Initialized, Locked>
where
    S: Ord + Clone,
    T: Clone,
{
    pub fn unlock(self) -> BTree<S, T, Initialized, Unlocked> {
        BTree {
            root: self.root,
            state: std::marker::PhantomData,
            lock_state: std::marker::PhantomData,
        }
    }
}

impl<S, T, LockState> BTree<S, T, Initialized, LockState>
where
    S: Ord + Clone,
    T: Clone,
{
    fn search_node(root: &Link<S, T>, key: &S) -> Option<T> {
        let (found, descend_into) = {
            let node = root.borrow();
            let mut i = 0;
            while i < node.keys.len() && key > &node.keys[i] {
                i += 1;
            }
            if i < node.keys.len() && key == &node.keys[i] {
                (Some(node.values[i].clone()), None)
            } else if node.is_leaf {
                (None, None)
            } else {
                (None, Some(node.children[i].clone()))
            }
        };
        if let Some(v) = found {
            return Some(v);
        }
        match descend_into {
            Some(child) => Self::search_node(&child, key),
            None => None,
        }
    }

    fn range_node(node: &Link<S, T>, result: &mut Vec<(S, T)>) {
        let node = node.borrow();
        for (key, value) in node.keys.iter().zip(node.values.iter()) {
            result.push((key.clone(), value.clone()));
        }
        if node.children.len() > 0 {
            for child in node.children.iter() {
                Self::range_node(child, result);
            }
        }
    }

    fn find_len(node: &Link<S, T>) -> usize {
        let node = node.borrow();
        node.keys.len() + node.children.iter().map(|c| Self::find_len(c)).sum::<usize>()
    }
    
    pub fn range(&self) -> Vec<(S, T)> {
        let mut result = Vec::new();
        Self::range_node(&self.root, &mut result);
        result
    }

    pub fn get(&self, key: &S) -> Option<T> {
        Self::search_node(&self.root, key)
    }

    pub fn len(&self) -> usize {
        Self::find_len(&self.root)
    }
}

impl<S, T> BTree<S, T, Initialized, Unlocked>
where
    S: Ord + Clone,
    T: Clone,
{
    fn insert(&mut self, key: S, value: T) {
        let root_is_full = self.root.borrow().is_full();

        if root_is_full {
            let new_root = Node::new_leaf();
            {
                let mut nr = new_root.borrow_mut();
                nr.children.push(self.root.clone());
                nr.is_leaf = false;
            }
            Self::split_child(&new_root, 0);
            self.root = new_root;
        }
        Self::insert_non_full(self.root.clone(), key, value);
    }

    fn split_child(parent: &Link<S, T>, i: usize) {
        let mid = MIN_DEGREE - 1;
        let child_link = parent.borrow().children[i].clone();

        let (mid_key, mid_val, right_keys, right_vals, right_children, child_is_leaf) = {
            let mut child = child_link.borrow_mut();
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
            (
                mid_key,
                mid_val,
                right_keys,
                right_vals,
                right_children,
                child.is_leaf,
            )
        };
        let right_node = Rc::new(RefCell::new(Node {
            keys: right_keys,
            values: right_vals,
            children: right_children,
            is_leaf: child_is_leaf,
        }));

        let mut parent_mut = parent.borrow_mut();
        parent_mut.keys.insert(i, mid_key);
        parent_mut.values.insert(i, mid_val);
        parent_mut.children.insert(i + 1, right_node);
    }

    fn insert_non_full(link: Link<S, T>, key: S, value: T) {
        let is_leaf = link.borrow().is_leaf;
        if is_leaf {
            let mut node = link.borrow_mut();
            let mut i = node.keys.len();
            while i > 0 && key < node.keys[i - 1] {
                i -= 1;
            }
            node.keys.insert(i, key);
            node.values.insert(i, value);
            return;
        }
        let (mut i, child_full) = {
            let node = link.borrow();
            let mut i = node.keys.len();
            while i > 0 && key < node.keys[i - 1] {
                i -= 1;
            }
            let child_full = node.children[i].borrow().is_full();
            (i, child_full)
        };
        if child_full {
            Self::split_child(&link, i);
            let promoted_key_bigger = key > link.borrow().keys[i];
            if promoted_key_bigger {
                i += 1;
            }
        }
        let child = link.borrow().children[i].clone();
        Self::insert_non_full(child, key, value);
    }

    fn delete_using_keys(&mut self, key: S) -> (bool, Option<T>) {
        let link = &mut self.root;
        let mut node = link.borrow_mut();

        if node.keys.len() == 0 {
            return (false, None);
        }
        let mut i = node.keys.len();
        while i > 0 && key < node.keys[i - 1] {
            i -= 1;
        }

        if i < node.keys.len() && key == node.keys[i] {
            node.keys.remove(i);
            let val = node.values.remove(i);
            if node.children.len() > 0 {
                node.children.remove(i);
            }
            return (true, Some(val));
        }
        (false, None)
    }

    pub fn lock(self) -> BTree<S, T, Initialized, Locked> {
        BTree {
            root: self.root,
            state: std::marker::PhantomData,
            lock_state: std::marker::PhantomData,
        }
    }

    pub fn put(&mut self, key: S, value: T) {
        Self::insert(self, key, value);
    }

    pub fn delete(&mut self, key: S) -> (bool, Option<T>) {
        Self::delete_using_keys(self, key)
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_insertion_structure() {
        let mut tree = BTree::new().unlock();
        tree.put(5, 10);
        tree.put(10, 100);
        tree.put(15, 150);
        tree.put(15, 150);
        tree.put(20, 200);
        tree.put(25, 250);

        let tree = tree.lock();
        
        let range = tree.range();
        let len = tree.len();
        assert_eq!(len, 5);
        assert_eq!(range, vec![(5, 10), (10, 100), (15, 150), (20, 200), (25, 250)]);

        assert_eq!(tree.get(&10), Some(100));
        assert_eq!(tree.get(&5), Some(10));
        assert_eq!(tree.get(&15), Some(150));
    }
}
