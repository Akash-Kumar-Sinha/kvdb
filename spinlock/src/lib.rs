//! A hand-rolled spin lock ([`SpinLock`]) built on [`std::sync::atomic::AtomicBool`],
//! used by `btree` to guard shared state across threads without reaching for
//! `std::sync::Mutex`.

mod spinlock;

pub use spinlock::{SpinLock, SpinLockGuard};
