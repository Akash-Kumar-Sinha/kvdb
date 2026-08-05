use btree::{BTree, Initialized, Locked, Uninitialized, Unlocked, Value, ValueError};
use scan::{Scan, ScanIter};
use serde::{Serialize, de::DeserializeOwned};

pub struct KvDb<S, LockState = Unlocked>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    inner: BTree<S, Initialized, LockState>,
}

impl<S> KvDb<S, Unlocked>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    pub fn open(path: &str) -> Self {
        let tree = BTree::<S, Uninitialized, Locked>::new(path);
        KvDb {
            inner: tree.unlock(),
        }
    }

    pub fn put(&mut self, key: S, value: impl Into<btree::Value>) {
        self.inner.put(key, value);
    }

    pub fn update(&mut self, key: S, value: impl Into<btree::Value>) {
        self.inner.update(key, value);
    }

    pub fn delete(&mut self, key: S) -> (bool, Option<Value>) {
        self.inner.delete(key)
    }

    pub fn lock(self) -> KvDb<S, Locked> {
        KvDb {
            inner: self.inner.lock(),
        }
    }
}

impl<S> KvDb<S, Locked>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    pub fn unlock(self) -> KvDb<S, Unlocked> {
        KvDb {
            inner: self.inner.unlock(),
        }
    }
}

impl<S, LockState> KvDb<S, LockState>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    pub fn get<R>(&mut self, key: &S) -> Result<R, ValueError>
    where
        R: TryFrom<Value, Error = ValueError>,
    {
        self.inner.get(key)
    }

    pub fn range(&mut self) -> Vec<(S, Value)> {
        self.inner.range()
    }

    pub fn len(&mut self) -> usize {
        self.inner.len()
    }

    #[must_use]
    pub fn is_empty(&mut self) -> bool {
        self.len() == 0
    }

    pub fn scan(&self) -> ScanIter<S> {
        self.inner.scan()
    }
}

impl<S, LockState> Clone for KvDb<S, LockState>
where
    S: Ord + Clone + Serialize + DeserializeOwned,
{
    fn clone(&self) -> Self {
        KvDb {
            inner: self.inner.clone(),
        }
    }
}

