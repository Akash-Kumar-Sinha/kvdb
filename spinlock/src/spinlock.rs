use std::cell::UnsafeCell;
use std::hint;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};

/// A mutual-exclusion lock built directly from an [`AtomicBool`], busy-waiting
/// instead of parking the thread.
///
/// KvDB's one use of this type guards `btree::PagerState`: every public entry
/// point on `BTree`/`KvDb` acquires it exactly once per call. Re-acquiring
/// while already holding it — even from a nested internal call — deadlocks,
/// since this lock is not reentrant.
///
/// # Examples
///
/// ```
/// use spinlock::SpinLock;
///
/// let lock = SpinLock::new(0);
/// {
///     let mut guard = lock.acquire();
///     *guard += 1;
/// } // guard dropped here, lock released
/// assert_eq!(*lock.acquire(), 1);
/// ```
pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

impl<T> SpinLock<T> {
    /// Wraps `value` in a new, unlocked `SpinLock`.
    pub fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    /// Blocks the calling thread — spinning, not parking — until the lock is
    /// free, then returns a guard granting exclusive access.
    ///
    /// The lock releases automatically when the returned [`SpinLockGuard`] drops.
    pub fn acquire(&self) -> SpinLockGuard<'_, T> {
        while self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            hint::spin_loop();
        }
        SpinLockGuard { lock: self }
    }
}

/// RAII guard granting exclusive access to a [`SpinLock`]'s contents.
///
/// Derefs to `&T`/`&mut T`; releases the lock automatically when dropped.
pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

// SAFETY: `SpinLock<T>` only exposes `T` through a `SpinLockGuard` obtained by
// `acquire`, which enforces exclusive access via the atomic `locked` flag —
// the same guarantee `std::sync::Mutex` relies on to be `Sync` for `T: Send`.
unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}
