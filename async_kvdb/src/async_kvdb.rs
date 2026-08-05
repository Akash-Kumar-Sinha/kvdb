use kvdb::{Codec, KvDb};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
};

use btree::{DbError, Locked, Unlocked, Value, ValueError};
use kvdb_rt::{KvdbCall, ThreadPoolHandle};

use crate::async_scan::AsyncScanIter;

type Job<R> = Box<dyn FnOnce() -> R + Send>;

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
    pub fn open(path: &str, num_workers: usize) -> Result<Self, DbError> {
        Ok(Self::with_db(KvDb::open(path)?, num_workers))
    }

    pub fn open_with_codec(
        path: &str,
        num_workers: usize,
        codec: Box<dyn Codec>,
    ) -> Result<Self, DbError> {
        Ok(Self::with_db(
            KvDb::open_with_codec(path, codec)?,
            num_workers,
        ))
    }

    fn with_db(inner: KvDb<S, Unlocked>, num_workers: usize) -> Self {
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

    pub fn put(&self, key: S, value: impl Into<Value>) -> KvdbCall<Result<(), DbError>> {
        let mut db = self.inner.clone();
        let value = value.into();
        let job: Job<Result<(), DbError>> = Box::new(move || db.put(key, value));
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn update(&self, key: S, value: impl Into<Value>) -> KvdbCall<Result<(), DbError>> {
        let mut db = self.inner.clone();
        let value = value.into();
        let job: Job<Result<(), DbError>> = Box::new(move || db.update(key, value));
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn delete(&self, key: S) -> KvdbCall<Result<(bool, Option<Value>), DbError>> {
        let mut db = self.inner.clone();
        let job: Job<Result<(bool, Option<Value>), DbError>> = Box::new(move || db.delete(key));
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
    pub fn get<R>(&self, key: S) -> KvdbCall<Result<R, DbError>>
    where
        R: TryFrom<Value, Error = ValueError> + Send + 'static,
    {
        let mut db = self.inner.clone();
        let job: Job<Result<R, DbError>> = Box::new(move || db.get::<R>(&key));
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn range(&self) -> KvdbCall<Result<Vec<(S, Value)>, DbError>> {
        let mut db = self.inner.clone();
        let job: Job<Result<Vec<(S, Value)>, DbError>> = Box::new(move || db.range());
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn len(&self) -> KvdbCall<Result<usize, DbError>> {
        let mut db = self.inner.clone();
        let job: Job<Result<usize, DbError>> = Box::new(move || db.len());
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn is_empty(&self) -> KvdbCall<Result<bool, DbError>> {
        let mut db = self.inner.clone();
        let job: Job<Result<bool, DbError>> = Box::new(move || db.is_empty());
        KvdbCall::new(job, self.pool.clone())
    }

    pub fn scan(&self) -> AsyncScanIter<S> {
        let (pager_state, stack) = self.inner.scan().into_parts();
        AsyncScanIter::new(pager_state, stack, self.pool.clone())
    }
}
