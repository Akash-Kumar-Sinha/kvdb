use btree::{BTree, DbError, Initialized, Locked, Uninitialized, Unlocked, Value, ValueError};
use codec::Codec;
use scan::{Scan, ScanIter};
use serde::{Serialize, de::DeserializeOwned};

/// KvDB's synchronous entry point: a typestate-checked handle over a
/// disk-backed B+tree.
///
/// `S` is the key type and must be `Ord + Clone + Serialize +
/// DeserializeOwned`. `LockState` (`Unlocked` by default) tracks at compile
/// time whether mutating methods (`put`, `delete`) exist on this handle —
/// see [`KvDb::lock`]/[`KvDb::unlock`].
///
/// Cloning a `KvDb` clones the *handle*, not the data: all clones share one
/// underlying spinlock-guarded pager, so handing a clone to another thread
/// shares the same database.
///
/// # Examples
///
/// ```
/// use kvdb::{KvDb, DbError};
///
/// # fn main() -> Result<(), DbError> {
/// # let path = "/tmp/kvdb_doctest_kvdb.db";
/// # std::fs::remove_file(path).ok();
/// let mut db = KvDb::<i32>::open(path)?;
/// db.put(1, "hello".to_string())?;
/// let value: String = db.get(&1)?;
/// assert_eq!(value, "hello");
/// # std::fs::remove_file(path).ok();
/// # Ok(())
/// # }
/// ```
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
    /// Opens (creating if absent) the database at `path`, using the default
    /// bincode wire format. Returns an **`Unlocked`** handle, so writes work
    /// immediately.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Io`] if the file cannot be opened.
    pub fn open(path: &str) -> Result<Self, DbError> {
        let tree = BTree::<S, Uninitialized, Locked>::new(path)?;
        Ok(KvDb {
            inner: tree.unlock(),
        })
    }

    /// The same as [`KvDb::open`], with an explicitly chosen wire format.
    ///
    /// # Errors
    ///
    /// Returns [`DbError::Io`] if the file cannot be opened.
    pub fn open_with_codec(path: &str, codec: Box<dyn Codec>) -> Result<Self, DbError> {
        let tree = BTree::<S, Uninitialized, Locked>::new_with_codec(path, codec)?;
        Ok(KvDb {
            inner: tree.unlock(),
        })
    }

    /// Inserts `value` under `key`.
    ///
    /// **Accumulates**, rather than overwrites, if `key` already holds a
    /// value: repeated `put`s of the same key fold every value into one
    /// `Value::Multi`, read back with `get::<Vec<Value>>`. Use [`KvDb::update`]
    /// for replace-in-place (upsert) semantics.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] on an I/O failure or if the entry would overflow
    /// the fixed page size.
    pub fn put(&mut self, key: S, value: impl Into<btree::Value>) -> Result<(), DbError> {
        self.inner.put(key, value)
    }

    /// Replaces the value at `key` if it exists (including discarding any
    /// accumulator `put` built there), otherwise inserts it.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] on an I/O failure or if the entry would overflow
    /// the fixed page size.
    pub fn update(&mut self, key: S, value: impl Into<btree::Value>) -> Result<(), DbError> {
        self.inner.update(key, value)
    }

    /// Removes `key` if present, returning `(true, Some(previous_value))`, or
    /// `(false, None)` if it was absent.
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] on an I/O failure.
    pub fn delete(&mut self, key: S) -> Result<(bool, Option<Value>), DbError> {
        self.inner.delete(key)
    }

    /// Consumes an `Unlocked` handle and returns a `Locked` one, at compile time — no I/O.
    ///
    /// A `Locked` handle has no `put`/`delete`/`update` methods at all — not
    /// a runtime check, but a compile error at the call site.
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
    /// Consumes a `Locked` handle and returns an `Unlocked` one, at compile time — no I/O.
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
    /// Looks up `key` and converts its value to `R`. Available on both `Locked` and `Unlocked` handles.
    ///
    /// # Errors
    ///
    /// Returns `DbError::Value(ValueError::NotFound)` if the key is absent,
    /// or `DbError::Value(ValueError::TypeMismatch)` if the stored value is
    /// not exactly `R` — conversions are exact-match, with no widening.
    pub fn get<R>(&mut self, key: &S) -> Result<R, DbError>
    where
        R: TryFrom<Value, Error = ValueError>,
    {
        self.inner.get(key)
    }

    /// Every entry, sorted by key, cloned into a fresh `Vec`.
    ///
    /// Holds the lock for the whole traversal, so this is always consistent
    /// even under concurrent writers — unlike [`KvDb::scan`].
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if a page cannot be read or decoded.
    pub fn range(&mut self) -> Result<Vec<(S, Value)>, DbError> {
        self.inner.range()
    }

    /// The number of entries in the database. Walks every leaf — not O(1).
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if a page cannot be read or decoded.
    pub fn len(&mut self) -> Result<usize, DbError> {
        self.inner.len()
    }

    /// `len() == 0`, with the same full-walk cost as [`KvDb::len`].
    ///
    /// # Errors
    ///
    /// Returns [`DbError`] if a page cannot be read or decoded.
    pub fn is_empty(&mut self) -> Result<bool, DbError> {
        Ok(self.len()? == 0)
    }

    /// A zero-copy, in-order cursor over every entry. Takes `&self`, not `&mut self`.
    ///
    /// Returns a [`ScanIter`] implementing `scan::LendingIterator`, not
    /// `std::iter::Iterator` — bring that trait into scope and drive it with
    /// `while let Some(item) = iter.next() { ... }`. Not snapshot-isolated: a
    /// concurrent writer can split a node mid-scan and the walk may skip or
    /// repeat an entry. [`KvDb::range`] is the consistent alternative.
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
