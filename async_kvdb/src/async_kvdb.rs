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

/// The async counterpart to [`KvDb`]: the same typestate-checked B+tree
/// handle, with every data method returning a [`KvdbCall`] future instead of
/// blocking the calling thread.
///
/// Wraps a `KvDb` rather than reimplementing it — each method clones the
/// inner handle (an `Arc` bump) and moves the clone into a closure dispatched
/// to a small worker-thread pool owned by this handle. `kvdb_rt` ships no
/// executor of its own, so awaiting a [`KvdbCall`] needs a real one (`tokio`,
/// `async-std`, or equivalent) driving the `Future`.
///
/// `LockState` mirrors `KvDb`'s typestate: mutating methods exist only on
/// `AsyncKvDb<S, Unlocked>`.
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
    /// Opens (creating if absent) the database at `path` and spawns
    /// `num_workers` worker threads that live as long as the returned handle.
    ///
    /// Opening itself runs on the calling thread — this returns a plain
    /// `Result`, not a future.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Io`] if the file cannot be opened.
    pub fn open(path: &str, num_workers: usize) -> Result<Self, DbError> {
        Ok(Self::with_db(KvDb::open(path)?, num_workers))
    }

    /// The same as [`AsyncKvDb::open`], with an explicitly chosen wire format.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Io`] if the file cannot be opened.
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

    /// Dispatches a [`KvDb::put`] to the pool. Does nothing until awaited.
    ///
    /// Accumulates on a repeat key, the same as the sync `put` — see
    /// `KvDb::put` for that behaviour.
    pub fn put(&self, key: S, value: impl Into<Value>) -> KvdbCall<Result<(), DbError>> {
        let mut db = self.inner.clone();
        let value = value.into();
        let job: Job<Result<(), DbError>> = Box::new(move || db.put(key, value));
        KvdbCall::new(job, self.pool.clone())
    }

    /// Dispatches a [`KvDb::update`] to the pool. Does nothing until awaited.
    pub fn update(&self, key: S, value: impl Into<Value>) -> KvdbCall<Result<(), DbError>> {
        let mut db = self.inner.clone();
        let value = value.into();
        let job: Job<Result<(), DbError>> = Box::new(move || db.update(key, value));
        KvdbCall::new(job, self.pool.clone())
    }

    /// Dispatches a [`KvDb::delete`] to the pool. Does nothing until awaited.
    pub fn delete(&self, key: S) -> KvdbCall<Result<(bool, Option<Value>), DbError>> {
        let mut db = self.inner.clone();
        let job: Job<Result<(bool, Option<Value>), DbError>> = Box::new(move || db.delete(key));
        KvdbCall::new(job, self.pool.clone())
    }

    /// Consumes an `Unlocked` handle and returns a `Locked` one, at compile time — no I/O.
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
    /// Consumes a `Locked` handle and returns an `Unlocked` one, at compile time — no I/O.
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
    /// Dispatches a [`KvDb::get`] to the pool. Does nothing until awaited.
    ///
    /// Takes `key` by value rather than by reference, unlike the sync
    /// `KvDb::get` — the job closure must be `Send + 'static` to cross into
    /// the pool, and a borrow tied to this call cannot satisfy that.
    pub fn get<R>(&self, key: S) -> KvdbCall<Result<R, DbError>>
    where
        R: TryFrom<Value, Error = ValueError> + Send + 'static,
    {
        let mut db = self.inner.clone();
        let job: Job<Result<R, DbError>> = Box::new(move || db.get::<R>(&key));
        KvdbCall::new(job, self.pool.clone())
    }

    /// Dispatches a [`KvDb::range`] to the pool. Does nothing until awaited.
    pub fn range(&self) -> KvdbCall<Result<Vec<(S, Value)>, DbError>> {
        let mut db = self.inner.clone();
        let job: Job<Result<Vec<(S, Value)>, DbError>> = Box::new(move || db.range());
        KvdbCall::new(job, self.pool.clone())
    }

    /// Dispatches a [`KvDb::len`] to the pool. Does nothing until awaited.
    pub fn len(&self) -> KvdbCall<Result<usize, DbError>> {
        let mut db = self.inner.clone();
        let job: Job<Result<usize, DbError>> = Box::new(move || db.len());
        KvdbCall::new(job, self.pool.clone())
    }

    /// Dispatches a [`KvDb::is_empty`] to the pool. Does nothing until awaited.
    pub fn is_empty(&self) -> KvdbCall<Result<bool, DbError>> {
        let mut db = self.inner.clone();
        let job: Job<Result<bool, DbError>> = Box::new(move || db.is_empty());
        KvdbCall::new(job, self.pool.clone())
    }

    /// Starts an async cursor over every entry, sorted by key.
    ///
    /// Unlike `KvDb::scan`'s borrowed `(&S, &Value)` items, `AsyncScanIter`
    /// yields owned `(S, Value)` — a job closure dispatched to the pool must
    /// be `Send + 'static`, which a borrow into this handle cannot satisfy.
    pub fn scan(&self) -> AsyncScanIter<S> {
        let (pager_state, cursor) = self.inner.scan().into_parts();
        AsyncScanIter::new(pager_state, cursor, self.pool.clone())
    }
}
