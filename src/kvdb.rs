use btree::{BTree, DbError, Initialized, Locked, Uninitialized, Unlocked, Value, ValueError};
use codec::Codec;
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
    pub fn open(path: &str) -> Result<Self, DbError> {
        let tree = BTree::<S, Uninitialized, Locked>::new(path)?;
        Ok(KvDb {
            inner: tree.unlock(),
        })
    }

    pub fn open_with_codec(path: &str, codec: Box<dyn Codec>) -> Result<Self, DbError> {
        let tree = BTree::<S, Uninitialized, Locked>::new_with_codec(path, codec)?;
        Ok(KvDb {
            inner: tree.unlock(),
        })
    }

    pub fn put(&mut self, key: S, value: impl Into<btree::Value>) -> Result<(), DbError> {
        self.inner.put(key, value)
    }

    pub fn update(&mut self, key: S, value: impl Into<btree::Value>) -> Result<(), DbError> {
        self.inner.update(key, value)
    }

    pub fn delete(&mut self, key: S) -> Result<(bool, Option<Value>), DbError> {
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
    pub fn get<R>(&mut self, key: &S) -> Result<R, DbError>
    where
        R: TryFrom<Value, Error = ValueError>,
    {
        self.inner.get(key)
    }

    pub fn range(&mut self) -> Result<Vec<(S, Value)>, DbError> {
        self.inner.range()
    }

    pub fn len(&mut self) -> Result<usize, DbError> {
        self.inner.len()
    }

    pub fn is_empty(&mut self) -> Result<bool, DbError> {
        Ok(self.len()? == 0)
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
