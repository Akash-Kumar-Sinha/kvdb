use kvdb::KvDb;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
};

use btree::{Locked, Unlocked, Value, ValueError};
use kvdb_rt::{KvdbCall, ThreadPoolHandle};

use crate::async_scan::AsyncScanIter;

pub struct AsyncKvDb<S, LockState = Unlocked>
where
    S: Ord + Clone + Serialize + DeserializeOwned + Send + 'static,
{
    inner: KvDb<S, LockState>,
    pool: ThreadPoolHandle,
}

impl<S> AsyncKvDb<S, Unlocked>
where
    S: Ord + Clone + Serialize + DeserializeOwned + Send + 'static,
{
    pub fn open(path: &str, num_workers: usize) -> Self {
        let inner = KvDb::open(path);
        let (sender, receiver) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
        let receiver = Arc::new(Mutex::new(receiver));

        for _ in 0..num_workers {
            let receiver = Arc::clone(&receiver);
            thread::spawn(move || {
                loop {
                    let job = match receiver.lock() {
                        Ok(rx) => rx.recv(),
                        Err(_) => break,
                    };
                    match job {
                        Ok(job) => job(),
                        Err(_) => break,
                    }
                }
            });
        }

        AsyncKvDb {
            inner,
            pool: sender,
        }
    }

    pub fn put(&self, key: S, value: impl Into<Value>) -> KvdbCall<()> {
        let mut db = self.inner.clone();
        let value = value.into();
        let job: Box<dyn FnOnce() + Send> = Box::new(move || {
            db.put(key, value);
        });
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn update(&self, key: S, value: impl Into<Value>) -> KvdbCall<()> {
        let mut db = self.inner.clone();
        let value = value.into();
        let job: Box<dyn FnOnce() + Send> = Box::new(move || {
            db.update(key, value);
        });
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn delete(&self, key: S) -> KvdbCall<(bool, Option<Value>)> {
        let mut db = self.inner.clone();
        let job: Box<dyn FnOnce() -> (bool, Option<Value>) + Send> =
            Box::new(move || db.delete(key));
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn lock(self) -> AsyncKvDb<S, Locked> {
        AsyncKvDb {
            inner: self.inner.lock(),
            pool: self.pool,
        }
    }
}

impl<S> AsyncKvDb<S, Locked>
where
    S: Ord + Clone + Serialize + DeserializeOwned + Send + 'static,
{
    pub fn unlock(self) -> AsyncKvDb<S, Unlocked> {
        AsyncKvDb {
            inner: self.inner.unlock(),
            pool: self.pool,
        }
    }
}

impl<S, LockState> AsyncKvDb<S, LockState>
where
    S: Ord + Clone + Serialize + DeserializeOwned + Send + 'static,
    LockState: Send + 'static,
{
    pub fn get<R>(&self, key: S) -> KvdbCall<Result<R, ValueError>>
    where
        R: TryFrom<Value, Error = ValueError> + Send + 'static,
    {
        let mut db = self.inner.clone();
        let job: Box<dyn FnOnce() -> Result<R, ValueError> + Send> =
            Box::new(move || db.get::<R>(&key));
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn range(&self) -> KvdbCall<Vec<(S, Value)>> {
        let mut db = self.inner.clone();
        let job: Box<dyn FnOnce() -> Vec<(S, Value)> + Send> = Box::new(move || db.range());
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn len(&self) -> KvdbCall<usize> {
        let mut db = self.inner.clone();
        let job: Box<dyn FnOnce() -> usize + Send> = Box::new(move || db.len());
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn is_empty(&self) -> KvdbCall<bool> {
        let mut db = self.inner.clone();
        let job: Box<dyn FnOnce() -> bool + Send> = Box::new(move || db.is_empty());
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn scan(&self) -> AsyncScanIter<S> {
        let (pager_state, stack) = self.inner.scan().into_parts();
        AsyncScanIter::new(pager_state, stack, self.pool.clone())
    }
}
